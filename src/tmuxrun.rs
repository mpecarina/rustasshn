use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use chrono::Local;
use directories::ProjectDirs;

pub type HasCredentialFn = Arc<dyn Fn(&str) -> bool + Send + Sync>;

#[derive(Clone, Default)]
pub struct Session {
    pub askpass_script: Option<PathBuf>,
    pub host_users: HashMap<String, String>,
    pub has_credential: Option<HasCredentialFn>,
}

pub fn in_tmux() -> bool {
    std::env::var("TMUX")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
}

fn socket_path() -> String {
    let v = std::env::var("TMUX").unwrap_or_default();
    let v = v.trim();
    if v.is_empty() {
        return String::new();
    }
    match v.find(',') {
        Some(i) => v[..i].to_string(),
        None => v.to_string(),
    }
}

impl Session {
    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new("tmux");
        let sock = socket_path();
        if !sock.is_empty() {
            cmd.arg("-S").arg(sock);
        }
        cmd.args(args);
        cmd
    }

    fn run(&self, args: &[&str]) -> Result<()> {
        let mut cmd = self.command(args);
        let output = cmd
            .output()
            .with_context(|| format!("tmux {}", args.join(" ")))?;
        if output.status.success() {
            return Ok(());
        }
        let msg = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let msg = if msg.is_empty() {
            format!("tmux {} failed", args.join(" "))
        } else {
            msg
        };
        bail!("tmux {}: {}", args.join(" "), msg)
    }

    fn output(&self, args: &[&str]) -> Result<String> {
        let mut cmd = self.command(args);
        let output = cmd
            .output()
            .with_context(|| format!("tmux {}", args.join(" ")))?;
        if !output.status.success() {
            let msg = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let msg = if msg.is_empty() {
                format!("tmux {} failed", args.join(" "))
            } else {
                msg
            };
            bail!("tmux {}: {}", args.join(" "), msg)
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    pub fn ssh_command_string(&self, alias: &str) -> String {
        if let (Some(script), Some(has)) = (&self.askpass_script, &self.has_credential)
            && has(alias)
        {
            let user = self.host_users.get(alias).cloned().unwrap_or_default();
            return format!(
                "export TSSM_HOST={} TSSM_USER={} SSH_ASKPASS={} SSH_ASKPASS_REQUIRE=force DISPLAY=1; exec ssh -o PubkeyAuthentication=no -o PreferredAuthentications=keyboard-interactive,password {}",
                shell_quote(alias),
                shell_quote(&user),
                shell_quote(&script.to_string_lossy()),
                shell_quote(alias)
            );
        }
        ssh_command(alias)
    }

    pub fn new_window(&self, alias: &str) -> Result<()> {
        let pane_id = self.output(&[
            "new-window",
            "-P",
            "-F",
            "#{pane_id}",
            "-n",
            alias,
            login_shell().as_str(),
            "-lc",
            self.ssh_command_string(alias).as_str(),
        ])?;
        self.setup_logging(&pane_id, alias);
        Ok(())
    }

    pub fn split_vertical(&self, alias: &str) -> Result<()> {
        let pane_id = self.output(&[
            "split-window",
            "-P",
            "-F",
            "#{pane_id}",
            "-v",
            "-c",
            "#{pane_current_path}",
            login_shell().as_str(),
            "-lc",
            self.ssh_command_string(alias).as_str(),
        ])?;
        self.setup_logging(&pane_id, alias);
        Ok(())
    }

    pub fn split_horizontal(&self, alias: &str) -> Result<()> {
        let pane_id = self.output(&[
            "split-window",
            "-P",
            "-F",
            "#{pane_id}",
            "-h",
            "-c",
            "#{pane_current_path}",
            login_shell().as_str(),
            "-lc",
            self.ssh_command_string(alias).as_str(),
        ])?;
        self.setup_logging(&pane_id, alias);
        Ok(())
    }

    pub fn tiled(&self, aliases: &[String], layout: &str) -> Result<()> {
        if aliases.is_empty() {
            return Ok(());
        }
        let layout = if layout.trim().is_empty() {
            "tiled"
        } else {
            layout
        };
        let window_id = self.output(&[
            "new-window",
            "-P",
            "-F",
            "#{window_id}",
            "-n",
            "tiled",
            login_shell().as_str(),
            "-lc",
            self.ssh_command_string(&aliases[0]).as_str(),
        ])?;

        if let Ok(pane_id) = self.output(&["display-message", "-p", "-t", &window_id, "#{pane_id}"])
        {
            self.setup_logging(&pane_id, &aliases[0]);
        }

        for alias in aliases.iter().skip(1) {
            let pane_id = self.output(&[
                "split-window",
                "-P",
                "-F",
                "#{pane_id}",
                "-v",
                "-t",
                &window_id,
                login_shell().as_str(),
                "-lc",
                self.ssh_command_string(alias).as_str(),
            ])?;
            self.setup_logging(&pane_id, alias);
            let _ = self.run(&["select-layout", "-t", &window_id, layout]);
        }
        let _ = self.run(&["select-layout", "-t", &window_id, layout]);
        Ok(())
    }

    pub fn setup_pane_logging(&self, alias: &str) {
        if !in_tmux() {
            return;
        }
        let pane_id = match self.output(&["display-message", "-p", "#{pane_id}"]) {
            Ok(v) => v,
            Err(_) => return,
        };
        self.setup_logging(&pane_id, alias);
    }

    fn setup_logging(&self, pane_id: &str, alias: &str) {
        if logging_disabled() {
            return;
        }
        let log_path = match ensure_log_file(alias) {
            Ok(p) => p,
            Err(_) => return,
        };
        let _ = self.run(&[
            "pipe-pane",
            "-O",
            "-t",
            pane_id,
            "-o",
            format!(
                "cat >> {} 2>/dev/null",
                shell_quote(&log_path.to_string_lossy())
            )
            .as_str(),
        ]);
    }
}

pub fn ssh_command(alias: &str) -> String {
    format!("exec ssh {}", shell_quote(alias))
}

fn login_shell() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "sh".to_string())
}

pub fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    let mut out = String::new();
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\"'\"'");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

fn logging_disabled() -> bool {
    match std::env::var("TSSM_DISABLE_LOGGING") {
        Ok(v) => matches!(
            v.trim().to_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => false,
    }
}

pub fn log_dir(alias: &str) -> Result<PathBuf> {
    let base = logs_base_dir()?;
    Ok(base.join(sanitize_alias(alias)))
}

fn logs_base_dir() -> Result<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(xdg).join("tmux-ssh-manager").join("logs"));
    }
    if let Some(proj) = ProjectDirs::from("", "", "tmux-ssh-manager") {
        return Ok(proj.config_dir().join("logs"));
    }
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow::anyhow!("resolve home"))?;
    Ok(PathBuf::from(home)
        .join(".config")
        .join("tmux-ssh-manager")
        .join("logs"))
}

fn ensure_log_file(alias: &str) -> Result<PathBuf> {
    let dir = log_dir(alias)?;
    fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
    }
    let filename = format!("{}.log", Local::now().format("%Y-%m-%d"));
    let path = dir.join(filename);
    let _f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(path)
}

pub fn sanitize_alias(s: &str) -> String {
    let mut out = String::new();
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() { "_".to_string() } else { out }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ssh_command_quotes_alias() {
        assert_eq!(ssh_command("prod'box"), "exec ssh 'prod'\"'\"'box'");
    }

    #[test]
    fn test_ssh_command_simple_alias() {
        assert_eq!(ssh_command("edge1"), "exec ssh 'edge1'");
    }

    #[test]
    fn test_ssh_command_empty_alias() {
        assert_eq!(ssh_command(""), "exec ssh ''");
    }

    #[test]
    fn test_session_ssh_command_disables_pubkey() {
        let mut s = Session::default();
        s.askpass_script = Some(PathBuf::from("/tmp/tssm-askpass.sh"));
        s.host_users.insert("edge1".into(), "admin".into());
        s.has_credential = Some(Arc::new(|a| a == "edge1"));
        let got = s.ssh_command_string("edge1");
        assert!(got.contains("PubkeyAuthentication=no"));
        assert!(got.contains("PreferredAuthentications=keyboard-interactive,password"));
    }

    #[test]
    fn test_session_ssh_command_without_credential() {
        let mut s = Session::default();
        s.askpass_script = Some(PathBuf::from("/tmp/tssm-askpass.sh"));
        s.host_users.insert("edge1".into(), "admin".into());
        s.has_credential = Some(Arc::new(|_| false));
        let got = s.ssh_command_string("edge1");
        assert!(!got.contains("PubkeyAuthentication"));
    }

    #[test]
    fn test_shell_quote() {
        let cases = [
            ("simple", "'simple'"),
            ("", "''"),
            ("it's", "'it'\"'\"'s'"),
            ("a b c", "'a b c'"),
            ("$VAR", "'$VAR'"),
        ];
        for (i, w) in cases {
            assert_eq!(shell_quote(i), w);
        }
    }

    #[test]
    fn test_in_tmux_reads_env() {
        unsafe { std::env::set_var("TMUX", "") };
        assert!(!in_tmux());
        unsafe { std::env::set_var("TMUX", "/tmp/tmux-501/default,12345,0") };
        assert!(in_tmux());
    }

    #[test]
    fn test_socket_path() {
        unsafe { std::env::set_var("TMUX", "/tmp/tmux-501/default,12345,0") };
        assert_eq!(socket_path(), "/tmp/tmux-501/default");
        unsafe { std::env::set_var("TMUX", "") };
        assert_eq!(socket_path(), "");
    }

    #[test]
    fn test_login_shell_fallback() {
        unsafe { std::env::set_var("SHELL", "") };
        assert_eq!(login_shell(), "sh");
        unsafe { std::env::set_var("SHELL", "/bin/zsh") };
        assert_eq!(login_shell(), "/bin/zsh");
    }

    #[test]
    fn test_sanitize_alias() {
        let cases = [
            ("edge1", "edge1"),
            ("my-host.local", "my-host.local"),
            ("user@host", "user_host"),
            ("host with spaces", "host_with_spaces"),
            ("host/path", "host_path"),
            ("", "_"),
            ("caf\u{00e9}", "caf_"),
        ];
        for (i, w) in cases {
            assert_eq!(sanitize_alias(i), w);
        }
    }

    #[test]
    fn test_log_dir_uses_xdg() {
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let got = log_dir("edge1").unwrap();
        assert_eq!(
            got,
            dir.path()
                .join("tmux-ssh-manager")
                .join("logs")
                .join("edge1")
        );
        unsafe { std::env::remove_var("XDG_CONFIG_HOME") };
    }

    #[test]
    fn test_ensure_log_file_creates_file() {
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let path = ensure_log_file("edge1").unwrap();
        assert!(path.exists());
        let md = fs::metadata(&path).unwrap();
        assert_eq!(md.len(), 0);
        unsafe { std::env::remove_var("XDG_CONFIG_HOME") };
    }
}

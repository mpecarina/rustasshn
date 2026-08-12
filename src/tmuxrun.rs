use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
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
        let sh = login_shell();
        if let (Some(script), Some(has)) = (&self.askpass_script, &self.has_credential)
            && has(alias)
        {
            let user = self.host_users.get(alias).cloned().unwrap_or_default();
            let (log_start, log_stop) = pipe_bracket(alias);
            return format!(
                "export TSSM_HOST={} TSSM_USER={} SSH_ASKPASS={} SSH_ASKPASS_REQUIRE=force DISPLAY=1; {}ssh -o PubkeyAuthentication=no -o PreferredAuthentications=keyboard-interactive,password {}; {}{}exec {} -l",
                shell_quote(alias),
                shell_quote(&user),
                shell_quote(&script.to_string_lossy()),
                log_start,
                shell_quote(alias),
                log_stop,
                reset_prefix(),
                shell_quote(&sh)
            );
        }
        ssh_command(alias)
    }

    pub fn new_window(&self, alias: &str) -> Result<()> {
        self.run(&[
            "new-window",
            "-n",
            alias,
            login_shell().as_str(),
            "-lc",
            self.ssh_command_string(alias).as_str(),
        ])
    }

    pub fn respawn_origin_pane(&self, alias: &str) -> Result<()> {
        if !in_tmux() {
            bail!("origin pane requires running inside tmux")
        }
        let origin_pane = std::env::var("RUSTASSHN_ORIGIN_PANE").unwrap_or_default();
        if origin_pane.trim().is_empty() {
            // Fallback: behave like "pane" (current pane).
            let cmd = self.ssh_command_string(alias);
            self.run(&["respawn-pane", "-k", "-c", "#{pane_current_path}", "--", login_shell().as_str(), "-lc", cmd.as_str()])?;
            return Ok(());
        }

        let origin_path = std::env::var("RUSTASSHN_ORIGIN_PATH").unwrap_or_default();
        let cwd = if origin_path.trim().is_empty() {
            "#{pane_current_path}"
        } else {
            origin_path.trim()
        };

        let cmd = self.ssh_command_string(alias);
        self.run(&[
            "respawn-pane",
            "-t",
            origin_pane.trim(),
            "-k",
            "-c",
            cwd,
            "--",
            login_shell().as_str(),
            "-lc",
            cmd.as_str(),
        ])?;
        Ok(())
    }

    pub fn split_vertical(&self, alias: &str) -> Result<()> {
        self.run(&[
            "split-window",
            "-v",
            "-c",
            "#{pane_current_path}",
            login_shell().as_str(),
            "-lc",
            self.ssh_command_string(alias).as_str(),
        ])
    }

    pub fn split_horizontal(&self, alias: &str) -> Result<()> {
        self.run(&[
            "split-window",
            "-h",
            "-c",
            "#{pane_current_path}",
            login_shell().as_str(),
            "-lc",
            self.ssh_command_string(alias).as_str(),
        ])
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

        for alias in aliases.iter().skip(1) {
            self.run(&[
                "split-window",
                "-v",
                "-t",
                &window_id,
                login_shell().as_str(),
                "-lc",
                self.ssh_command_string(alias).as_str(),
            ])?;
            let _ = self.run(&["select-layout", "-t", &window_id, layout]);
        }
        let _ = self.run(&["select-layout", "-t", &window_id, layout]);
        Ok(())
    }
}

fn pipe_sink(log_path: &Path) -> String {
    format!(
        "cat >> {} 2>/dev/null",
        shell_quote(&log_path.to_string_lossy())
    )
}

/// Shell fragments that open and close pane logging from *inside* the pane, so
/// the pipe covers exactly the ssh session: it closes when ssh exits rather than
/// following the login shell that replaces it, it captures from the session's
/// first byte, and the pane names its own target through `$TMUX_PANE` instead of
/// leaving tmux to resolve one.
///
/// Both fragments are empty when logging is off, so we never close a pipe the
/// user opened themselves.
fn pipe_bracket(alias: &str) -> (String, String) {
    if logging_disabled() {
        return (String::new(), String::new());
    }
    let Ok(log_path) = ensure_log_file(alias) else {
        return (String::new(), String::new());
    };
    (pipe_start_cmd(&log_path), pipe_stop_cmd())
}

fn pipe_start_cmd(log_path: &Path) -> String {
    // `-o` is deliberately absent: it toggles, so a second call would close an
    // existing pipe rather than replace it.
    format!(
        "tmux pipe-pane -O -t \"$TMUX_PANE\" {}; ",
        shell_quote(&pipe_sink(log_path))
    )
}

fn pipe_stop_cmd() -> String {
    "tmux pipe-pane -t \"$TMUX_PANE\"; ".to_string()
}

pub fn ssh_command(alias: &str) -> String {
    let sh = login_shell();
    let (log_start, log_stop) = pipe_bracket(alias);
    // Logging stops before the reset, so the reset's escape bytes stay out of
    // the log file.
    format!(
        "{}ssh {}; {}{}exec {} -l",
        log_start,
        shell_quote(alias),
        log_stop,
        reset_prefix(),
        shell_quote(&sh)
    )
}

/// The pane's login shell starts the instant ssh exits, with no gap in which to
/// notice a terminal the remote left mangled. Undo that damage in between.
fn reset_prefix() -> String {
    match std::env::current_exe() {
        Ok(p) => format!("{} __reset; ", shell_quote(&p.to_string_lossy())),
        Err(_) => String::new(),
    }
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
        return Ok(PathBuf::from(xdg).join("rustasshn").join("logs"));
    }
    if let Some(proj) = ProjectDirs::from("", "", "rustasshn") {
        return Ok(proj.config_dir().join("logs"));
    }
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow::anyhow!("resolve home"))?;
    Ok(PathBuf::from(home)
        .join(".config")
        .join("rustasshn")
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

    /// The command builders consult logging policy, and with logging on they
    /// create the log file as a side effect. Turning it off keeps these tests
    /// off the filesystem and independent of `XDG_CONFIG_HOME`, which other
    /// tests in this process mutate.
    fn without_logging() {
        unsafe { std::env::set_var("TSSM_DISABLE_LOGGING", "1") };
    }

    #[test]
    fn test_ssh_command_quotes_alias() {
        without_logging();
        assert_eq!(
            ssh_command("prod'box"),
            format!("ssh 'prod'\"'\"'box'; {}exec '/bin/zsh' -l", reset_prefix())
        );
    }

    #[test]
    fn test_ssh_command_simple_alias() {
        without_logging();
        assert_eq!(
            ssh_command("edge1"),
            format!("ssh 'edge1'; {}exec '/bin/zsh' -l", reset_prefix())
        );
    }

    #[test]
    fn test_ssh_command_empty_alias() {
        without_logging();
        assert_eq!(
            ssh_command(""),
            format!("ssh ''; {}exec '/bin/zsh' -l", reset_prefix())
        );
    }

    #[test]
    fn test_logging_disabled_emits_no_bracket() {
        without_logging();
        assert_eq!(pipe_bracket("edge1"), (String::new(), String::new()));
        assert!(!ssh_command("edge1").contains("pipe-pane"));
    }

    #[test]
    fn test_pipe_start_cmd_targets_own_pane_and_quotes_path() {
        let got = pipe_start_cmd(Path::new("/logs/edge 1/it's.log"));
        assert_eq!(
            got,
            "tmux pipe-pane -O -t \"$TMUX_PANE\" 'cat >> '\"'\"'/logs/edge 1/it'\"'\"'\"'\"'\"'\"'\"'\"'s.log'\"'\"' 2>/dev/null'; "
        );
        // `$TMUX_PANE` must stay expandable by the pane's shell.
        assert!(got.contains("\"$TMUX_PANE\""));
        // `-o` toggles rather than replaces, so it must not appear.
        assert!(!got.contains(" -o "));
    }

    #[test]
    fn test_pipe_stop_cmd_closes_pipe_for_own_pane() {
        assert_eq!(pipe_stop_cmd(), "tmux pipe-pane -t \"$TMUX_PANE\"; ");
    }

    #[test]
    fn test_pane_command_brackets_ssh_with_logging() {
        // Assembled directly so the test does not depend on logging policy.
        let (start, stop) = (pipe_start_cmd(Path::new("/tmp/x.log")), pipe_stop_cmd());
        let got = format!(
            "{start}ssh 'edge1'; {stop}{}exec '/bin/zsh' -l",
            reset_prefix()
        );

        let open = got.find("pipe-pane -O").expect("no pipe open");
        let ssh = got.find("ssh 'edge1'").expect("no ssh");
        let close = got.rfind("pipe-pane -t").expect("no pipe close");
        let reset = got.find("__reset").expect("no reset");
        let shell = got.find("exec ").expect("no exec");

        // Open before ssh so the session is captured from its first byte;
        // close before the reset so reset bytes stay out of the log.
        assert!(
            open < ssh && ssh < close && close < reset && reset < shell,
            "wrong order: {got}"
        );
    }

    #[test]
    fn test_reset_prefix_is_quoted_and_trailing_separated() {
        let p = reset_prefix();
        assert!(p.ends_with("__reset; "), "unexpected prefix: {p:?}");
        assert!(p.starts_with('\''), "exe path must be quoted: {p:?}");
    }

    #[test]
    fn test_ssh_command_resets_between_ssh_and_shell() {
        let got = ssh_command("edge1");
        let reset = got.find("__reset").expect("reset step missing");
        let ssh = got.find("ssh ").expect("ssh missing");
        let shell = got.find("exec ").expect("exec missing");
        assert!(ssh < reset && reset < shell, "wrong order: {got}");
    }

    #[test]
    fn test_session_ssh_command_resets_on_askpass_path() {
        let mut s = Session::default();
        s.askpass_script = Some(PathBuf::from("/tmp/tssm-askpass.sh"));
        s.host_users.insert("edge1".into(), "admin".into());
        s.has_credential = Some(Arc::new(|a| a == "edge1"));
        let got = s.ssh_command_string("edge1");
        let reset = got.find("__reset").expect("reset step missing");
        let shell = got.find("exec ").expect("exec missing");
        assert!(reset < shell, "wrong order: {got}");
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
        assert_eq!(got, dir.path().join("rustasshn").join("logs").join("edge1"));
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

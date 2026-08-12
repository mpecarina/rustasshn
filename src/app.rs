use std::ffi::OsString;
use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, ExitStatus};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};

use crate::credentials;
use crate::sshconfig;
use crate::state;
use crate::termio;
use crate::tmuxrun;
use crate::tty;
use crate::ui;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Parser, Debug)]
#[command(name = "rustasshn")]
#[command(version = VERSION)]
#[command(disable_help_subcommand = true)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,

    #[arg(long, short = 'm', default_value = "search", global = true)]
    mode: String,

    #[arg(long, default_value_t = true, global = true)]
    implicit_select: bool,

    #[arg(long, default_value = DEFAULT_ENTER_MODE, global = true)]
    enter_mode: String,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    List(ListArgs),
    Connect(ConnectArgs),
    Add(AddArgs),
    Cred(CredArgs),
    #[command(name = "__askpass")]
    Askpass(AskpassArgs),
    #[command(name = "__reset")]
    Reset,
    Ssh(PassthroughArgs),
    Scp(PassthroughArgs),
    #[command(name = "print-ssh-config-path")]
    PrintSshConfigPath,
}

#[derive(Args, Debug)]
struct ListArgs {
    #[arg(long, default_value_t = false)]
    json: bool,
}

#[derive(Args, Debug)]
struct ConnectArgs {
    alias: String,
    #[arg(long, default_value_t = false)]
    dry_run: bool,
    #[arg(long, default_value_t = 0)]
    split_count: i32,
    #[arg(long, default_value = "window")]
    split_mode: String,
    #[arg(long, default_value = "")]
    layout: String,
}

#[derive(Args, Debug)]
struct AddArgs {
    #[arg(long)]
    alias: String,
    #[arg(long)]
    hostname: Option<String>,
    #[arg(long)]
    user: Option<String>,
    #[arg(long, default_value_t = 0)]
    port: i32,
    #[arg(long)]
    proxyjump: Option<String>,
    #[arg(long)]
    identity_file: Option<String>,
}

#[derive(Args, Debug)]
struct CredArgs {
    #[command(subcommand)]
    action: CredAction,
}

#[derive(Subcommand, Debug)]
enum CredAction {
    Set(CredFlags),
    Get(CredFlags),
    Delete(CredFlags),
}

#[derive(Args, Debug, Clone)]
struct CredFlags {
    #[arg(long)]
    host: String,
    #[arg(long, default_value = "")]
    user: String,
    #[arg(long, default_value = "password")]
    kind: String,
}

#[derive(Args, Debug)]
struct AskpassArgs {
    #[arg(long)]
    host: String,
    #[arg(long, default_value = "")]
    user: String,
    #[arg(long, default_value = "password")]
    kind: String,
    #[arg(long, default_value = "")]
    prompt: String,
}

#[derive(Args, Debug)]
struct PassthroughArgs {
    #[arg(allow_hyphen_values = true, trailing_var_arg = true)]
    args: Vec<OsString>,
}

pub fn run<I>(args: I) -> Result<()>
where
    I: IntoIterator<Item = OsString>,
{
    let cli = Cli::parse_from(args);

    match cli.cmd {
        Some(Cmd::List(a)) => run_list(a),
        Some(Cmd::Connect(a)) => run_connect(a),
        Some(Cmd::Add(a)) => run_add(a),
        Some(Cmd::Cred(a)) => run_cred(a),
        Some(Cmd::Askpass(a)) => run_askpass(a),
        Some(Cmd::Reset) => termio::restore_after_child(),
        Some(Cmd::Ssh(a)) => run_ssh_passthrough("ssh", a),
        Some(Cmd::Scp(a)) => run_ssh_passthrough("scp", a),
        Some(Cmd::PrintSshConfigPath) => {
            let path = sshconfig::default_primary_path()?;
            println!("{}", path.display());
            Ok(())
        }
        None => run_picker(cli),
    }
}

/// Every mode spawns the session in a real tmux pane, which is what makes the
/// session loggable and keeps a remote that dies mid-escape-sequence from
/// reaching the terminal directly. Unrecognized input takes the default.
pub(crate) fn normalize_enter_mode(raw: &str) -> &str {
    match raw.trim().to_lowercase().as_str() {
        "o" | "origin" => "o",
        "w" | "window" => "w",
        "s" | "split" | "split-h" => "s",
        "v" | "split-v" => "v",
        _ => DEFAULT_ENTER_MODE,
    }
}

/// Origin pane: the session opens where the picker was invoked from. This
/// respawns that pane, killing whatever was running there. Override with
/// `set -g @rustasshn_enter_mode 'w'|'v'|'s'` in tmux.conf.
pub(crate) const DEFAULT_ENTER_MODE: &str = "o";

fn run_list(args: ListArgs) -> Result<()> {
    let hosts = sshconfig::load_default()?;
    if args.json {
        #[derive(serde::Serialize)]
        struct Entry {
            alias: String,
            #[serde(skip_serializing_if = "String::is_empty")]
            hostname: String,
            #[serde(skip_serializing_if = "String::is_empty")]
            user: String,
            #[serde(skip_serializing_if = "Option::is_none")]
            port: Option<i32>,
            #[serde(skip_serializing_if = "String::is_empty")]
            proxyjump: String,
            #[serde(skip_serializing_if = "Vec::is_empty")]
            identity_files: Vec<String>,
        }

        let entries: Vec<Entry> = hosts
            .into_iter()
            .map(|h| Entry {
                alias: h.alias,
                hostname: h.hostname,
                user: h.user,
                port: if h.port > 0 { Some(h.port) } else { None },
                proxyjump: h.proxyjump,
                identity_files: h.identity_files,
            })
            .collect();
        let out = serde_json::to_string_pretty(&entries)?;
        println!("{out}");
        return Ok(());
    }
    for h in hosts {
        println!("{}", h.alias);
    }
    Ok(())
}

fn run_connect(args: ConnectArgs) -> Result<()> {
    let alias = args.alias.trim().to_string();
    if alias.is_empty() {
        bail!(
            "usage: rustasshn connect [--dry-run] [--split-count N] [--split-mode window|v|h] [--layout tiled] <alias>"
        )
    }
    if args.dry_run {
        println!("ssh {alias}");
        return Ok(());
    }

    if args.split_count > 1 {
        return run_connect_split(&alias, args.split_count, &args.split_mode, &args.layout);
    }

    // If a password is stored for this host, force askpass even with a TTY.
    // This is critical for the zsh `tssm-run` flow which uses `connect`.
    let host_users = sshconfig::load_default()
        .ok()
        .map(|hosts| {
            hosts
                .into_iter()
                .map(|h| (h.alias, h.user))
                .collect::<std::collections::HashMap<String, String>>()
        })
        .unwrap_or_default();

    let user = host_users.get(&alias).cloned().unwrap_or_default();
    let askpass_path = ensure_askpass_script()?;
    let mut cmd = if credentials::get(&alias, &user, "password").is_ok()
        && let Some(script_path) = askpass_path.as_ref()
    {
        let mut c = Command::new("ssh");
        c.arg("-o")
            .arg("PubkeyAuthentication=no")
            .arg("-o")
            .arg("PreferredAuthentications=keyboard-interactive,password")
            .arg(&alias);
        c.env("TSSM_HOST", &alias)
            .env("TSSM_USER", &user)
            .env("SSH_ASKPASS", script_path)
            .env("SSH_ASKPASS_REQUIRE", "force")
            .env("DISPLAY", "1");
        c
    } else {
        let mut c = Command::new("ssh");
        c.arg(&alias);
        c
    };

    // `connect` is what the zsh `tssm-run` wrapper calls, so stale input from
    // that wrapper should not reach ssh.
    termio::sanitize_stdin_before_exec().ok();
    let status = tty::run_child(&mut cmd).with_context(|| format!("exec ssh {alias}"))?;
    exit_from_status(status)
}

fn run_connect_split(alias: &str, count: i32, mode: &str, layout: &str) -> Result<()> {
    if !tmuxrun::in_tmux() {
        bail!("split-count requires running inside tmux")
    }
    let mode = mode.trim().to_lowercase();
    let mode = if mode.is_empty() {
        "window".to_string()
    } else {
        mode
    };
    let s = tmuxrun::Session::default();
    match mode.as_str() {
        "window" => {
            for _ in 0..count {
                s.new_window(alias)?;
            }
            Ok(())
        }
        "v" | "h" => {
            let mut aliases = Vec::new();
            for _ in 0..count {
                aliases.push(alias.to_string());
            }
            let layout = if layout.trim().is_empty() {
                "tiled".to_string()
            } else {
                layout.trim().to_string()
            };
            s.tiled(&aliases, &layout)
        }
        _ => bail!("split-mode must be one of: window, v, h"),
    }
}

fn run_add(args: AddArgs) -> Result<()> {
    let input = sshconfig::AddHostInput {
        alias: args.alias,
        hostname: args.hostname.unwrap_or_default(),
        user: args.user.unwrap_or_default(),
        port: args.port,
        proxyjump: args.proxyjump.unwrap_or_default(),
        identity_file: args.identity_file.unwrap_or_default(),
    };
    sshconfig::add_host_to_primary(input.clone())?;
    println!("added host {}", input.alias.trim());
    Ok(())
}

fn run_cred(args: CredArgs) -> Result<()> {
    match args.action {
        CredAction::Set(f) => {
            credentials::set(&f.host, &f.user, &f.kind)?;
            println!("stored {} for {}", f.kind.trim(), subject(&f.host, &f.user));
            Ok(())
        }
        CredAction::Get(f) => {
            credentials::get(&f.host, &f.user, &f.kind)?;
            println!("{} exists for {}", f.kind.trim(), subject(&f.host, &f.user));
            Ok(())
        }
        CredAction::Delete(f) => {
            credentials::delete(&f.host, &f.user, &f.kind)?;
            println!(
                "deleted {} for {}",
                f.kind.trim(),
                subject(&f.host, &f.user)
            );
            Ok(())
        }
    }
}

fn subject(host: &str, user: &str) -> String {
    let host = host.trim();
    let user = user.trim();
    if !user.is_empty() {
        format!("{}@{}", user, host)
    } else {
        host.to_string()
    }
}

fn run_askpass(args: AskpassArgs) -> Result<()> {
    if args.host.trim().is_empty() {
        bail!(
            "usage: rustasshn __askpass --host <alias> [--user <user>] [--kind password] [--prompt <prompt>]"
        )
    }

    let env_host = args.host.trim().to_string();
    let mut host = env_host.clone();
    let mut user = args.user.trim().to_string();

    if let Some(t) = parse_askpass_prompt_target(&args.prompt) {
        user = t.user;
        if let Ok(hosts) = sshconfig::load_default() {
            host = resolve_prompt_host_with_hosts(&env_host, &t.host, &hosts);
        } else {
            host = t.host;
        }
    }

    match credentials::reveal(&host, &user, &args.kind) {
        Ok(secret) => {
            print!("{secret}");
        }
        Err(e) => {
            // Avoid spamming stderr during ssh retry loops unless explicitly requested.
            if std::env::var("RUSTASSHN_ASKPASS_DEBUG")
                .ok()
                .map(|v| !v.trim().is_empty() && v.trim() != "0")
                .unwrap_or(false)
            {
                eprintln!("{e}");
            }
        }
    }
    io::stdout().flush().ok();
    Ok(())
}

pub(crate) fn normalize_prompt_host(host: &str) -> String {
    let mut h = host
        .trim()
        .trim_end_matches(':')
        .to_string();

    if let Some((left, right)) = h.rsplit_once(':')
        && !left.contains(':')
        && !left.is_empty()
        && !right.is_empty()
        && right.bytes().all(|b| b.is_ascii_digit())
    {
        h = left.to_string();
    }

    h = h
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    h
}

pub(crate) fn resolve_prompt_host_with_hosts(
    env_host: &str,
    prompt_host: &str,
    hosts: &[sshconfig::Host],
) -> String {
    let env_host = env_host.trim();
    let ph = normalize_prompt_host(prompt_host);
    if ph.is_empty() {
        return env_host.to_string();
    }
    if ph == env_host {
        return env_host.to_string();
    }

    if let Some(env_entry) = hosts.iter().find(|h| h.alias == env_host) {
        if !env_entry.hostname.trim().is_empty() && env_entry.hostname.trim() == ph {
            return env_host.to_string();
        }
    }

    if hosts.iter().any(|h| h.alias == ph) {
        return ph;
    }

    let mut matches = hosts.iter().filter(|h| h.hostname.trim() == ph).map(|h| h.alias.as_str());
    let first = matches.next();
    if first.is_none() {
        return ph;
    }

    let mut best = first.unwrap().to_string();
    for a in matches {
        if a == env_host {
            best = env_host.to_string();
            break;
        }
    }
    best
}

pub(crate) fn parse_askpass_prompt_target(prompt: &str) -> Option<SshCredentialTarget> {
    let p = prompt.trim();
    if p.is_empty() {
        return None;
    }

    // OpenSSH typically calls SSH_ASKPASS with a prompt containing a token like:
    // "user@host's password:". We try to extract the most plausible "user@host".
    let bytes = p.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b != b'@' {
            continue;
        }

        let mut l = i;
        while l > 0 {
            let c = bytes[l - 1];
            if (c as char).is_ascii_alphanumeric() || c == b'.' || c == b'_' || c == b'-' {
                l -= 1;
            } else {
                break;
            }
        }

        let mut r = i + 1;
        while r < bytes.len() {
            let c = bytes[r];
            if (c as char).is_ascii_alphanumeric()
                || c == b'.'
                || c == b'_'
                || c == b'-'
                || c == b':'
                || c == b'['
                || c == b']'
                || c == b'%'
            {
                r += 1;
            } else {
                break;
            }
        }

        if l >= i || r <= i + 1 {
            continue;
        }
        let token = &p[l..r];
        if token.starts_with('@') || token.ends_with('@') {
            continue;
        }
        let (host, user) = host_and_user_from_destination(token);
        if host.trim().is_empty() || user.trim().is_empty() {
            continue;
        }

        let host = host
            .trim()
            .trim_end_matches(':')
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_string();
        return Some(SshCredentialTarget { host, user });
    }

    None
}

fn run_picker(cli: Cli) -> Result<()> {
    // Every connect mode spawns a tmux pane, so outside tmux the picker has
    // nothing it can safely do. Refusing here beats offering an action that
    // would hand the bare terminal to ssh and log nothing.
    if !tmuxrun::in_tmux() {
        bail!(
            "the host picker requires tmux: start a tmux session first.\n\
             To connect without tmux, use `rustasshn connect <alias>`, or `rustasshn ssh ...` / `rustasshn scp ...`."
        )
    }

    let hosts = sshconfig::load_default()?;
    let state_path = state::default_path()?;
    let store = state::load(&state_path)?;

    let mut host_users = std::collections::HashMap::new();
    for h in &hosts {
        host_users.insert(h.alias.clone(), h.user.clone());
    }

    let askpass_path = ensure_askpass_script()?;

    let has_cred_map = host_users.clone();
    let has_cred = move |alias: &str| {
        let user = has_cred_map.get(alias).cloned().unwrap_or_default();
        credentials::get(alias, &user, "password").is_ok()
    };

    let sess = tmuxrun::Session {
        askpass_script: askpass_path.clone(),
        host_users: host_users.clone(),
        has_credential: Some(Arc::new(move |a: &str| has_cred(a))),
    };

    let start_in_search = cli.mode.trim().to_lowercase() != "normal";
    let enter_mode = normalize_enter_mode(&cli.enter_mode).to_string();

    let sess_new = sess.clone();
    let sess_v = sess.clone();
    let sess_h = sess.clone();
    let sess_t = sess.clone();
    let sess_o = sess.clone();
    let app = ui::AppConfig {
        hosts,
        store,
        state_path,
        start_in_search,
        implicit_select: cli.implicit_select,
        enter_mode,
        add_host: sshconfig::add_host_to_primary,
        exec_credential: credential_command,
        new_window: Arc::new(move |alias| sess_new.new_window(alias)),
        split_vert: Arc::new(move |alias| sess_v.split_vertical(alias)),
        split_horiz: Arc::new(move |alias| sess_h.split_horizontal(alias)),
        respawn_origin: Arc::new(move |alias| sess_o.respawn_origin_pane(alias)),
        tiled: Arc::new(move |aliases, layout| sess_t.tiled(aliases, layout)),
    };

    ui::run(app)
}

fn credential_command(action: &str, host: &str, user: &str, kind: &str) -> Result<Command> {
    let exe = std::env::current_exe().context("resolve executable")?;
    Ok(credential_command_for_path(&exe, action, host, user, kind))
}

pub(crate) fn credential_command_for_path(
    path: &Path,
    action: &str,
    host: &str,
    user: &str,
    kind: &str,
) -> Command {
    let mut cmd = Command::new(path);
    cmd.arg("cred")
        .arg(action)
        .arg("--host")
        .arg(host.trim())
        .arg("--kind")
        .arg(kind.trim());
    if !user.trim().is_empty() {
        cmd.arg("--user").arg(user.trim());
    }
    cmd
}

fn shell_quote(s: &str) -> String {
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

fn ensure_askpass_script() -> Result<Option<std::path::PathBuf>> {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return Ok(None),
    };

    let home = std::env::var_os("HOME").ok_or_else(|| anyhow::anyhow!("resolve home"))?;
    let dir = std::path::PathBuf::from(home)
        .join(".config")
        .join("rustasshn");
    std::fs::create_dir_all(&dir).ok();

    let path = dir.join("askpass.sh");
    let content = format!(
        "#!/usr/bin/env bash\nexec {} __askpass --host \"$TSSM_HOST\" --user \"$TSSM_USER\" --kind password --prompt \"$1\"\n",
        shell_quote(&exe.to_string_lossy())
    );
    let write = match std::fs::read_to_string(&path) {
        Ok(existing) => existing != content,
        Err(_) => true,
    };
    if write {
        std::fs::write(&path, content.as_bytes())?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700));
    }
    Ok(Some(path))
}

fn run_ssh_passthrough(binary: &str, args: PassthroughArgs) -> Result<()> {
    let bin_path = which::which(binary).with_context(|| format!("{binary} not found in PATH"))?;
    let mut cmd = Command::new(&bin_path);
    cmd.args(&args.args);

    if let Some(dest) = extract_ssh_credential_target(binary, &args.args)
        && let Ok(hosts) = sshconfig::load_default()
    {
        let host_users: std::collections::HashMap<String, String> =
            hosts.into_iter().map(|h| (h.alias, h.user)).collect();

        if let Some(user) = resolve_credential_user(
            &dest.host,
            &dest.user,
            host_users.get(&dest.host).map(|s| s.as_str()),
        ) && let Some(script_path) = ensure_askpass_script()?.as_ref()
        {
            let mut askpass_args: Vec<OsString> = vec![
                OsString::from("-o"),
                OsString::from("PubkeyAuthentication=no"),
                OsString::from("-o"),
                OsString::from("PreferredAuthentications=keyboard-interactive,password"),
            ];
            askpass_args.extend(args.args.iter().cloned());
            cmd = Command::new(&bin_path);
            cmd.args(&askpass_args);
            cmd.env("TSSM_HOST", &dest.host)
                .env("TSSM_USER", user)
                .env("SSH_ASKPASS", script_path)
                .env("SSH_ASKPASS_REQUIRE", "force")
                .env("DISPLAY", "1");

            let status = tty::run_child(&mut cmd)?;
            return exit_from_status(status);
        }
    }

    let status = tty::run_child(&mut cmd)?;
    exit_from_status(status)
}

#[derive(Debug, Clone)]
pub(crate) struct SshCredentialTarget {
    pub(crate) host: String,
    pub(crate) user: String,
}

pub(crate) fn extract_ssh_credential_target(
    binary: &str,
    args: &[OsString],
) -> Option<SshCredentialTarget> {
    // Mirrors Go logic: parse -l, -o User=..., -oUser=..., stop at --.
    let mut cli_user = String::new();
    let consumes_next: std::collections::HashSet<&'static str> = [
        "-b", "-c", "-D", "-E", "-e", "-F", "-I", "-i", "-J", "-L", "-l", "-m", "-O", "-o", "-p",
        "-Q", "-R", "-S", "-W", "-w",
    ]
    .into_iter()
    .collect();

    let mut i = 0;
    while i < args.len() {
        let a = args[i].to_string_lossy();
        if a == "--" {
            break;
        }
        if a.starts_with("-oUser=") {
            cli_user = a.trim_start_matches("-oUser=").to_string();
            i += 1;
            continue;
        }
        if a.starts_with('-') {
            if a == "-l" && i + 1 < args.len() {
                cli_user = args[i + 1].to_string_lossy().to_string();
            }
            if a == "-o"
                && i + 1 < args.len()
                && let Some((k, v)) = split_ssh_option(&args[i + 1].to_string_lossy())
                && k.eq_ignore_ascii_case("User")
            {
                cli_user = v;
            }
            if consumes_next.contains(a.as_ref()) && i + 1 < args.len() {
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }

        if binary == "scp" {
            if let Some(idx) = a.find(':')
                && idx > 0
            {
                let (host, user) = host_and_user_from_destination(&a[..idx]);
                let user = if !cli_user.is_empty() && user.is_empty() {
                    cli_user.clone()
                } else {
                    user
                };
                return Some(SshCredentialTarget { host, user });
            }
            i += 1;
            continue;
        }

        let (host, user) = host_and_user_from_destination(&a);
        let user = if !cli_user.is_empty() && user.is_empty() {
            cli_user.clone()
        } else {
            user
        };
        return Some(SshCredentialTarget { host, user });
    }
    None
}

fn host_and_user_from_destination(dest: &str) -> (String, String) {
    if let Some(at) = dest.rfind('@') {
        (dest[at + 1..].to_string(), dest[..at].to_string())
    } else {
        (dest.to_string(), String::new())
    }
}

fn split_ssh_option(option: &str) -> Option<(String, String)> {
    let mut parts = option.splitn(2, '=');
    let k = parts.next()?.trim();
    let v = parts.next()?.trim();
    if k.is_empty() {
        return None;
    }
    Some((k.to_string(), v.to_string()))
}

fn resolve_credential_user(
    host: &str,
    candidates: &str,
    from_config: Option<&str>,
) -> Option<String> {
    let mut seen = std::collections::HashSet::<String>::new();
    let mut list = Vec::new();
    if !candidates.trim().is_empty() {
        list.push(candidates.trim().to_string());
    }
    if let Some(u) = from_config
        && !u.trim().is_empty()
    {
        list.push(u.trim().to_string());
    }
    list.push(String::new());
    for c in list {
        if !seen.insert(c.clone()) {
            continue;
        }
        if credentials::get(host, &c, "password").is_ok() {
            return Some(c);
        }
    }
    None
}

fn exit_from_status(status: ExitStatus) -> Result<()> {
    if status.success() {
        Ok(())
    } else {
        // Preserve exit code as best we can.
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            if let Some(code) = status.code() {
                std::process::exit(code);
            }
            if let Some(sig) = status.signal() {
                std::process::exit(128 + sig);
            }
        }
        bail!("command failed")
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use clap::Parser;

    use super::{Cli, Cmd};

    #[test]
    fn test_scp_passthrough_accepts_recursive_flag_without_double_dash() {
        let cli = Cli::try_parse_from([
            "rustasshn",
            "scp",
            "-r",
            "../2026_06_01",
            "192.168.0.160:/Users/mattp/Desktop/",
        ])
        .unwrap();

        let Some(Cmd::Scp(args)) = cli.cmd else {
            panic!("expected scp subcommand");
        };

        assert_eq!(args.args[0], OsString::from("-r"));
        assert_eq!(args.args[1], OsString::from("../2026_06_01"));
        assert_eq!(
            args.args[2],
            OsString::from("192.168.0.160:/Users/mattp/Desktop/")
        );
    }
}

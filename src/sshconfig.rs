use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, BufRead};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Host {
    pub alias: String,
    pub hostname: String,
    pub user: String,
    pub port: i32,
    pub proxyjump: String,
    pub identity_files: Vec<String>,
    pub source_path: String,
    pub source_line: usize,
}

#[derive(Debug, Clone, Default)]
pub struct AddHostInput {
    pub alias: String,
    pub hostname: String,
    pub user: String,
    pub port: i32,
    pub proxyjump: String,
    pub identity_file: String,
}

pub fn default_primary_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow::anyhow!("resolve home"))?;
    Ok(PathBuf::from(home).join(".ssh").join("config"))
}

pub fn load_default() -> Result<Vec<Host>> {
    let p = default_primary_path()?;
    load(&[p])
}

pub fn load(paths: &[PathBuf]) -> Result<Vec<Host>> {
    if paths.is_empty() {
        bail!("no ssh config paths provided");
    }

    let mut visited: HashSet<PathBuf> = HashSet::new();
    let mut merged: HashMap<String, Host> = HashMap::new();
    let mut order: Vec<String> = Vec::new();

    for p in paths {
        let p = expand_pathbuf(p);
        let entries = parse_recursive(&p, &mut visited)?;
        for e in entries {
            if !merged.contains_key(&e.alias) {
                order.push(e.alias.clone());
            }
            merged.insert(e.alias.clone(), e);
        }
    }

    let mut out: Vec<Host> = Vec::with_capacity(merged.len());
    for alias in order {
        if let Some(h) = merged.get(&alias) {
            out.push(h.clone());
        }
    }
    out.sort_by(|a, b| a.alias.cmp(&b.alias));
    Ok(out)
}

pub fn add_host_to_primary(input: AddHostInput) -> Result<()> {
    let p = default_primary_path()?;
    add_host(&p, input)
}

pub fn add_host(path: &Path, mut input: AddHostInput) -> Result<()> {
    input.alias = input.alias.trim().to_string();
    input.hostname = input.hostname.trim().to_string();
    input.user = input.user.trim().to_string();
    input.proxyjump = input.proxyjump.trim().to_string();
    input.identity_file = input.identity_file.trim().to_string();

    if input.alias.is_empty() {
        bail!("alias is required");
    }
    if input.hostname.is_empty() {
        input.hostname = input.alias.clone();
    }

    let existing = load(&[expand_pathbuf(path)]);
    if let Ok(current) = existing {
        for h in current {
            if h.alias == input.alias {
                bail!("host alias already exists: {}", input.alias);
            }
        }
    }

    let path = expand_pathbuf(path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| "create ssh config dir")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }
    }

    let mut builder = String::new();
    if let Ok(data) = fs::read_to_string(&path) {
        builder.push_str(&data);
        if !builder.ends_with('\n') && !builder.is_empty() {
            builder.push('\n');
        }
        builder.push('\n');
    }

    builder.push_str("Host ");
    builder.push_str(&input.alias);
    builder.push('\n');
    builder.push_str("  HostName ");
    builder.push_str(&input.hostname);
    builder.push('\n');
    if !input.user.is_empty() {
        builder.push_str("  User ");
        builder.push_str(&input.user);
        builder.push('\n');
    }
    if input.port > 0 {
        builder.push_str("  Port ");
        builder.push_str(&input.port.to_string());
        builder.push('\n');
    }
    if !input.proxyjump.is_empty() {
        builder.push_str("  ProxyJump ");
        builder.push_str(&input.proxyjump);
        builder.push('\n');
    }
    if !input.identity_file.is_empty() {
        builder.push_str("  IdentityFile ");
        builder.push_str(&input.identity_file);
        builder.push('\n');
    }

    let tmp = path.with_extension("tmp");
    fs::write(&tmp, builder.as_bytes()).with_context(|| "write ssh config")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
    }
    fs::rename(&tmp, &path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct HostBlock {
    patterns: Vec<String>,
    settings: HashMap<String, Vec<String>>,
    source: String,
    start_line: usize,
}

fn parse_recursive(path: &Path, visited: &mut HashSet<PathBuf>) -> Result<Vec<Host>> {
    let path = expand_pathbuf(path);
    let abs = path.canonicalize().unwrap_or(path);
    if visited.contains(&abs) {
        return Ok(Vec::new());
    }
    visited.insert(abs.clone());

    let f = match fs::File::open(&abs) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("open ssh config {}", abs.display())),
    };
    let reader = io::BufReader::new(f);

    let mut out: Vec<Host> = Vec::new();
    let mut current: Option<HostBlock> = None;
    let flush = |out: &mut Vec<Host>, current: &mut Option<HostBlock>| {
        if let Some(b) = current.take() {
            out.extend(b.to_hosts());
        }
    };

    for (idx, line) in reader.lines().enumerate() {
        let line_no = idx + 1;
        let line = line?;
        let line = strip_inline_comment(&line);
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let Some((key, value)) = split_directive(line) else {
            continue;
        };
        match key.to_lowercase().as_str() {
            "host" => {
                flush(&mut out, &mut current);
                current = Some(HostBlock {
                    patterns: value.split_whitespace().map(|s| s.to_string()).collect(),
                    settings: HashMap::new(),
                    source: abs.to_string_lossy().to_string(),
                    start_line: line_no,
                });
            }
            "include" => {
                flush(&mut out, &mut current);
                for p in expand_includes(&abs, &value) {
                    let mut entries = parse_recursive(&p, visited)?;
                    out.append(&mut entries);
                }
            }
            "match" => {
                flush(&mut out, &mut current);
            }
            _ => {
                let Some(b) = current.as_mut() else {
                    continue;
                };
                let k = key.trim().to_lowercase();
                let v = value.trim().to_string();
                if k == "identityfile" {
                    b.settings.entry(k).or_default().push(v);
                } else {
                    b.settings.insert(k, vec![v]);
                }
            }
        }
    }

    flush(&mut out, &mut current);
    Ok(out)
}

impl HostBlock {
    fn last(&self, key: &str) -> String {
        self.settings
            .get(key)
            .and_then(|v| v.last())
            .cloned()
            .unwrap_or_default()
    }

    fn to_hosts(&self) -> Vec<Host> {
        let mut hosts = Vec::new();
        for p in &self.patterns {
            let p = p.trim();
            if !is_literal_pattern(p) {
                continue;
            }
            hosts.push(Host {
                alias: p.to_string(),
                hostname: self.last("hostname"),
                user: self.last("user"),
                port: parse_port(&self.last("port")),
                proxyjump: self.last("proxyjump"),
                identity_files: self
                    .settings
                    .get("identityfile")
                    .cloned()
                    .unwrap_or_default(),
                source_path: self.source.clone(),
                source_line: self.start_line,
            });
        }
        hosts
    }
}

pub fn parse_port(value: &str) -> i32 {
    let v = value.trim();
    if v.is_empty() {
        return 0;
    }
    match v.parse::<i32>() {
        Ok(p) if p > 0 => p,
        _ => 0,
    }
}

pub fn is_literal_pattern(pattern: &str) -> bool {
    let p = pattern.trim();
    if p.is_empty() || p.starts_with('!') {
        return false;
    }
    !p.contains('*') && !p.contains('?') && !p.contains('[') && !p.contains(']')
}

pub fn strip_inline_comment(line: &str) -> String {
    let mut out = String::new();
    let mut in_single = false;
    let mut in_double = false;
    for ch in line.chars() {
        match ch {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '#' if !in_single && !in_double => {
                return out.trim_end_matches([' ', '\t']).to_string();
            }
            _ => {}
        }
        out.push(ch);
    }
    out
}

pub fn split_directive(line: &str) -> Option<(String, String)> {
    if line.trim().is_empty() {
        return None;
    }
    let idx = line
        .find(|c: char| [' ', '\t', '='].contains(&c))
        .unwrap_or(usize::MAX);
    if idx == usize::MAX {
        return None;
    }
    let key = line[..idx].trim();
    let value = line[idx + 1..].trim();
    if key.is_empty() {
        return None;
    }
    Some((key.to_string(), value.to_string()))
}

fn expand_includes(base_file: &Path, raw: &str) -> Vec<PathBuf> {
    let raw = expand_path(raw);
    let raw = raw.trim();
    if raw.is_empty() {
        return Vec::new();
    }
    let mut pattern = PathBuf::from(raw);
    if !pattern.is_absolute()
        && let Some(dir) = base_file.parent()
    {
        pattern = dir.join(pattern);
    }
    let glob_pat = pattern.to_string_lossy().to_string();
    let mut matches: Vec<PathBuf> = Vec::new();
    if let Ok(paths) = glob::glob(&glob_pat) {
        for p in paths.flatten() {
            matches.push(p);
        }
    }
    matches.sort();
    matches
}

fn expand_pathbuf(p: &Path) -> PathBuf {
    PathBuf::from(expand_path(&p.to_string_lossy()))
}

fn expand_path(path: &str) -> String {
    let path = path.trim();
    if path.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    let mut chars = path.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '$' {
            let mut name = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_ascii_alphanumeric() || c == '_' {
                    name.push(c);
                    chars.next();
                } else {
                    break;
                }
            }
            if !name.is_empty()
                && let Ok(v) = std::env::var(&name)
            {
                out.push_str(&v);
                continue;
            }
            out.push('$');
            out.push_str(&name);
            continue;
        }
        out.push(ch);
    }
    if out == "~"
        && let Ok(home) = std::env::var("HOME")
    {
        return home;
    }
    if out.starts_with("~/")
        && let Ok(home) = std::env::var("HOME")
    {
        return PathBuf::from(home)
            .join(out.trim_start_matches("~/"))
            .to_string_lossy()
            .to_string();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_load_includes_and_last_wins() {
        let root = tempfile::tempdir().unwrap();
        let ssh_dir = root.path().join(".ssh");
        fs::create_dir_all(ssh_dir.join("conf.d")).unwrap();
        let primary = ssh_dir.join("config");
        fs::write(
            &primary,
            "Include conf.d/*.conf\n\nHost app\n  HostName app-primary\n  User alice\n\nHost wildcard-*\n  HostName ignored\n",
        )
        .unwrap();
        fs::write(
            ssh_dir.join("conf.d").join("base.conf"),
            "Host db\n  HostName db.internal\n  User bob\n  Port 2222\n",
        )
        .unwrap();

        let hosts = load(&[primary]).unwrap();
        assert_eq!(hosts.len(), 2);
        assert!(
            hosts
                .iter()
                .any(|h| h.alias == "app" && h.hostname == "app-primary")
        );
        assert!(
            hosts
                .iter()
                .any(|h| h.alias == "db" && h.port == 2222 && h.user == "bob")
        );
    }

    #[test]
    fn test_add_host_appends_block() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config");
        fs::write(&path, "Host old\n  HostName old.example\n").unwrap();
        add_host(
            &path,
            AddHostInput {
                alias: "newbox".into(),
                hostname: "10.0.0.10".into(),
                user: "matt".into(),
                port: 2201,
                proxyjump: "bastion".into(),
                identity_file: "~/.ssh/id_ed25519".into(),
            },
        )
        .unwrap();
        let content = fs::read_to_string(&path).unwrap();
        for want in [
            "Host newbox",
            "HostName 10.0.0.10",
            "User matt",
            "Port 2201",
            "ProxyJump bastion",
            "IdentityFile ~/.ssh/id_ed25519",
        ] {
            assert!(content.contains(want), "missing {want} in:\n{content}");
        }
    }

    #[test]
    fn test_add_host_rejects_duplicate() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config");
        fs::write(&path, "Host existing\n  HostName 1.2.3.4\n").unwrap();
        let err = add_host(
            &path,
            AddHostInput {
                alias: "existing".into(),
                hostname: "5.6.7.8".into(),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("already exists"));
    }

    #[test]
    fn test_add_host_requires_alias() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config");
        let err = add_host(
            &path,
            AddHostInput {
                hostname: "1.2.3.4".into(),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("alias is required"));
    }

    #[test]
    fn test_inline_comment_stripping() {
        let cases = [
            ("HostName 1.2.3.4 # production", "HostName 1.2.3.4"),
            ("HostName 1.2.3.4", "HostName 1.2.3.4"),
            (
                "HostName \"foo#bar\" # real comment",
                "HostName \"foo#bar\"",
            ),
            ("# full line comment", ""),
            ("HostName 'has#hash'", "HostName 'has#hash'"),
        ];
        for (input, want) in cases {
            assert_eq!(strip_inline_comment(input), want);
        }
    }

    #[test]
    fn test_parse_port() {
        let cases = [
            ("22", 22),
            ("2222", 2222),
            ("", 0),
            ("abc", 0),
            ("-1", 0),
            ("0", 0),
            ("  3000  ", 3000),
        ];
        for (i, w) in cases {
            assert_eq!(parse_port(i), w);
        }
    }

    #[test]
    fn test_is_literal_pattern() {
        let cases = [
            ("myhost", true),
            ("my-host.example.com", true),
            ("*", false),
            ("web-*", false),
            ("!negated", false),
            ("host[1-3]", false),
            ("host?", false),
            ("", false),
        ];
        for (i, w) in cases {
            assert_eq!(is_literal_pattern(i), w);
        }
    }

    #[test]
    fn test_multiple_identity_files() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("config");
        fs::write(
            &path,
            "Host multi\n  HostName 1.2.3.4\n  IdentityFile ~/.ssh/id_rsa\n  IdentityFile ~/.ssh/id_ed25519\n",
        )
        .unwrap();
        let hosts = load(&[path]).unwrap();
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].identity_files.len(), 2);
    }

    #[test]
    fn test_load_missing_file() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("nonexistent");
        let hosts = load(&[path]).unwrap();
        assert_eq!(hosts.len(), 0);
    }

    #[test]
    fn test_split_directive() {
        let cases = [
            (
                "HostName 1.2.3.4",
                Some(("HostName".to_string(), "1.2.3.4".to_string())),
            ),
            ("Port=2222", Some(("Port".to_string(), "2222".to_string()))),
            (
                "User\tadmin",
                Some(("User".to_string(), "admin".to_string())),
            ),
            ("", None),
            ("single", None),
        ];
        for (input, want) in cases {
            assert_eq!(split_directive(input), want);
        }
    }
}

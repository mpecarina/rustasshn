use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

const RECENTS_LIMIT: usize = 100;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Store {
    #[serde(default = "default_version")]
    pub version: i32,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub favorites: Vec<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recents: Vec<String>,

    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub updated_at: String,
}

fn default_version() -> i32 {
    1
}

impl Default for Store {
    fn default() -> Self {
        Self {
            version: 1,
            favorites: Vec::new(),
            recents: Vec::new(),
            updated_at: String::new(),
        }
    }
}

pub fn default_path() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(p).join("tmux-ssh-manager").join("state.json"));
    }
    if let Some(proj) = ProjectDirs::from("", "", "tmux-ssh-manager") {
        return Ok(proj.config_dir().join("state.json"));
    }
    let home = std::env::var_os("HOME").ok_or_else(|| anyhow::anyhow!("resolve home"))?;
    Ok(PathBuf::from(home)
        .join(".config")
        .join("tmux-ssh-manager")
        .join("state.json"))
}

pub fn load(path: &Path) -> Result<Store> {
    let data = match fs::read(path) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Store::default()),
        Err(e) => return Err(e).with_context(|| "read state"),
    };
    let mut store: Store = serde_json::from_slice(&data).with_context(|| "parse state")?;
    if store.version == 0 {
        store.version = 1;
    }
    store.normalize();
    Ok(store)
}

pub fn save(path: &Path, store: &mut Store) -> Result<()> {
    if store.version == 0 {
        store.version = 1;
    }
    store.normalize();
    let now: DateTime<Utc> = Utc::now();
    store.updated_at = now.to_rfc3339();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| "create state dir")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }
    }
    let mut data = serde_json::to_vec_pretty(store).with_context(|| "encode state")?;
    data.push(b'\n');
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, &data).with_context(|| "write tmp state")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
    }
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

impl Store {
    pub fn toggle_favorite(&mut self, alias: &str) -> bool {
        let alias = alias.trim();
        if alias.is_empty() {
            return false;
        }
        if self.is_favorite(alias) {
            self.favorites.retain(|a| a != alias);
            return false;
        }
        self.favorites.push(alias.to_string());
        self.normalize();
        true
    }

    pub fn is_favorite(&self, alias: &str) -> bool {
        let alias = alias.trim();
        self.favorites.iter().any(|a| a == alias)
    }

    pub fn add_recent(&mut self, alias: &str) {
        let alias = alias.trim();
        if alias.is_empty() {
            return;
        }
        let mut next: Vec<String> = Vec::with_capacity(self.recents.len() + 1);
        next.push(alias.to_string());
        for item in &self.recents {
            if item != alias {
                next.push(item.clone());
            }
        }
        if next.len() > RECENTS_LIMIT {
            next.truncate(RECENTS_LIMIT);
        }
        self.recents = next;
    }

    fn normalize(&mut self) {
        if self.version == 0 {
            self.version = 1;
        }
        self.favorites = unique_non_empty(&self.favorites);
        self.recents = unique_non_empty(&self.recents);
        if self.recents.len() > RECENTS_LIMIT {
            self.recents.truncate(RECENTS_LIMIT);
        }
    }
}

fn unique_non_empty(items: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::<String>::new();
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let t = item.trim();
        if t.is_empty() {
            continue;
        }
        if !seen.insert(t.to_string()) {
            continue;
        }
        out.push(t.to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_toggle_favorite_and_recents() {
        let mut s = Store::default();
        assert!(s.toggle_favorite("edge1"));
        assert!(s.is_favorite("edge1"));
        assert!(!s.toggle_favorite("edge1"));
        s.add_recent("one");
        s.add_recent("two");
        s.add_recent("one");
        assert_eq!(s.recents[0], "one");
        assert_eq!(s.recents.len(), 2);
    }

    #[test]
    fn test_save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let mut s = Store::default();
        s.toggle_favorite("host-a");
        s.toggle_favorite("host-b");
        s.add_recent("host-c");
        s.add_recent("host-a");
        save(&path, &mut s).unwrap();
        let loaded = load(&path).unwrap();
        assert!(loaded.is_favorite("host-a"));
        assert!(loaded.is_favorite("host-b"));
        assert_eq!(loaded.recents.len(), 2);
        assert_eq!(loaded.recents[0], "host-a");
        assert!(!loaded.updated_at.is_empty());
    }

    #[test]
    fn test_load_missing_file_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist.json");
        let s = load(&path).unwrap();
        assert_eq!(s.version, 1);
        assert!(s.favorites.is_empty());
        assert!(s.recents.is_empty());
    }

    #[test]
    fn test_recents_cap_at_100() {
        let mut s = Store::default();
        for i in 0..150 {
            s.add_recent(&format!("host-{}", i));
        }
        assert!(s.recents.len() <= RECENTS_LIMIT);
    }

    #[test]
    fn test_toggle_favorite_ignores_empty() {
        let mut s = Store::default();
        assert!(!s.toggle_favorite(""));
        assert!(!s.toggle_favorite("  "));
    }

    #[test]
    fn test_default_path_uses_xdg() {
        let dir = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        let p = default_path().unwrap();
        assert!(p.is_absolute());
        assert!(p.starts_with(dir.path()));
        unsafe { std::env::remove_var("XDG_CONFIG_HOME") };
    }

    #[test]
    fn test_save_creates_directory() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("nested").join("dir");
        let path = nested.join("state.json");
        let mut s = Store::default();
        save(&path, &mut s).unwrap();
        assert!(path.exists());
    }
}

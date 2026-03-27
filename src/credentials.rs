use anyhow::{Context, Result, bail};

const SERVICE_PREFIX: &str = "rustasshn";

fn normalize_host(host: &str) -> Result<String> {
    let h = host.trim();
    if h.is_empty() {
        bail!("host is required");
    }
    Ok(h.to_string())
}

fn normalize_user(host: &str, user: &str) -> String {
    let u = user.trim();
    if !u.is_empty() {
        return u.to_string();
    }
    host.to_string()
}

fn normalize_kind(kind: &str) -> String {
    match kind.trim().to_lowercase().as_str() {
        "" | "password" => "password".to_string(),
        "passphrase" => "passphrase".to_string(),
        "otp" | "totp" => "otp".to_string(),
        other => other.to_string(),
    }
}

fn service_name(host: &str, kind: &str) -> String {
    format!("{}:{}:{}", SERVICE_PREFIX, host, normalize_kind(kind))
}

fn subject_label(host: &str, user: &str) -> String {
    if !user.is_empty() && user != host {
        return format!("{}@{}", user, host);
    }
    host.to_string()
}

fn item_label(host: &str, user: &str, kind: &str) -> String {
    let k = normalize_kind(kind);
    if !user.is_empty() && user != host {
        return format!("{} for {}@{}", k, user, host);
    }
    format!("{} for {}", k, host)
}

#[cfg(target_os = "macos")]
pub fn set(host: &str, user: &str, kind: &str) -> Result<()> {
    let host = normalize_host(host)?;
    let kind = normalize_kind(kind);
    let user = normalize_user(&host, user);
    let secret = prompt_secret(&format!(
        "Enter {} for {}",
        kind,
        subject_label(&host, &user)
    ))?;
    if secret.trim().is_empty() {
        bail!("empty secret refused");
    }
    run_security(&[
        "add-generic-password",
        "-U",
        "-s",
        &service_name(&host, &kind),
        "-a",
        &user,
        "-l",
        &item_label(&host, &user, &kind),
        "-w",
        &secret,
    ])
    .context("keychain write failed")?;
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn get(host: &str, user: &str, kind: &str) -> Result<()> {
    let host = normalize_host(host)?;
    let user = normalize_user(&host, user);
    let kind = normalize_kind(kind);

    run_security(&[
        "find-generic-password",
        "-s",
        &service_name(&host, &kind),
        "-a",
        &user,
    ])
    .map(|_| ())
    .map_err(|_| {
        anyhow::anyhow!(
            "credential not found for {}",
            item_label(&host, &user, &kind)
        )
    })
}

#[cfg(target_os = "macos")]
pub fn delete(host: &str, user: &str, kind: &str) -> Result<()> {
    let host = normalize_host(host)?;
    let user = normalize_user(&host, user);
    let kind = normalize_kind(kind);
    run_security(&[
        "delete-generic-password",
        "-s",
        &service_name(&host, &kind),
        "-a",
        &user,
    ])
    .context("keychain delete failed")?;
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn reveal(host: &str, user: &str, kind: &str) -> Result<String> {
    let host = normalize_host(host)?;
    let user = normalize_user(&host, user);
    let kind = normalize_kind(kind);
    let out = run_security(&[
        "find-generic-password",
        "-w",
        "-s",
        &service_name(&host, &kind),
        "-a",
        &user,
    ])
    .map_err(|_| {
        anyhow::anyhow!(
            "credential not found for {}",
            item_label(&host, &user, &kind)
        )
    })?;
    let secret = out.trim_end_matches(['\r', '\n']).to_string();
    if secret.is_empty() {
        bail!("empty credential for {}", item_label(&host, &user, &kind));
    }
    Ok(secret)
}

#[cfg(not(target_os = "macos"))]
pub fn set(_host: &str, _user: &str, _kind: &str) -> Result<()> {
    bail!("credentials are only supported on macOS")
}

#[cfg(not(target_os = "macos"))]
pub fn get(_host: &str, _user: &str, _kind: &str) -> Result<()> {
    bail!("credentials are only supported on macOS")
}

#[cfg(not(target_os = "macos"))]
pub fn delete(_host: &str, _user: &str, _kind: &str) -> Result<()> {
    bail!("credentials are only supported on macOS")
}

#[cfg(not(target_os = "macos"))]
pub fn reveal(_host: &str, _user: &str, _kind: &str) -> Result<String> {
    bail!("credentials are only supported on macOS")
}

#[cfg(target_os = "macos")]
fn run_security(args: &[&str]) -> Result<String> {
    let path = if std::path::Path::new("/usr/bin/security").exists() {
        "/usr/bin/security"
    } else {
        "security"
    };
    let out = std::process::Command::new(path)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .with_context(|| "run security")?;
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if msg.is_empty() {
            bail!("security failed")
        }
        bail!("{msg}")
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

#[cfg(target_os = "macos")]
fn prompt_secret(prompt: &str) -> Result<String> {
    let p = format!("{}: ", prompt);
    let s = rpassword::prompt_password(p).with_context(|| "read password")?;
    Ok(s.trim_end_matches(['\r', '\n']).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_kind() {
        assert_eq!(normalize_kind(""), "password");
        assert_eq!(normalize_kind("password"), "password");
        assert_eq!(normalize_kind("totp"), "otp");
        assert_eq!(normalize_kind("passphrase"), "passphrase");
    }

    #[test]
    fn test_service_name() {
        assert_eq!(
            service_name("edge1", "password"),
            "rustasshn:edge1:password"
        );
    }
}

//! `vudo --update`: replace the running binary with the latest GitHub release
//! build for this platform. Mirrors install.sh but in-process, so updating is
//! a single command. The SHA-256 is verified before the binary is swapped.
//!
//! Downloads/extraction shell out to curl/wget/tar/sha256sum (already required
//! by install.sh) to keep the binary dependency-free.

const REPO: &str = "elleryfamilia/vudo";

pub fn run() -> i32 {
    let current = env!("CARGO_PKG_VERSION");
    let latest = match latest_tag() {
        Some(t) => t,
        None => {
            eprintln!("vudo: could not determine the latest release (network?)");
            return 1;
        }
    };
    let latest_ver = latest.trim_start_matches('v');
    if !is_newer(current, latest_ver) {
        println!("vudo is already up to date (v{current}; latest is {latest}).");
        return 0;
    }
    println!("vudo: updating v{current} -> {latest} ...");
    match apply(&latest) {
        Ok(path) => {
            println!("vudo: updated to {latest} at {path}");
            0
        }
        Err(e) => {
            eprintln!("vudo: update failed: {e}");
            1
        }
    }
}

fn latest_tag() -> Option<String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    parse_tag(&http_get(&url)?)
}

/// Is `latest` a strictly newer version than `current`? Both are dotted
/// numeric versions ("0.3.0"). If either can't be parsed, fall back to a
/// plain inequality so an update is still offered.
fn is_newer(current: &str, latest: &str) -> bool {
    match (parse_ver(current), parse_ver(latest)) {
        (Some(c), Some(l)) => l > c,
        _ => current != latest,
    }
}

fn parse_ver(s: &str) -> Option<(u64, u64, u64)> {
    let mut it = s.split('.');
    let a = it.next()?.parse().ok()?;
    let b = it.next()?.parse().ok()?;
    let c = it.next().unwrap_or("0").parse().ok()?;
    Some((a, b, c))
}

/// Extract the `tag_name` value from a GitHub release JSON payload.
fn parse_tag(body: &str) -> Option<String> {
    let key = "\"tag_name\"";
    let rest = &body[body.find(key)? + key.len()..];
    let start = rest.find('"')? + 1;
    let end = rest[start..].find('"')? + start;
    Some(rest[start..end].to_string())
}

fn http_get(url: &str) -> Option<String> {
    use std::process::Command;
    if let Ok(o) = Command::new("curl")
        .args(["-fsSL", "-H", "User-Agent: vudo-update", url])
        .output()
    {
        if o.status.success() {
            return Some(String::from_utf8_lossy(&o.stdout).into_owned());
        }
    }
    if let Ok(o) = Command::new("wget")
        .args(["-qO-", "--header=User-Agent: vudo-update", url])
        .output()
    {
        if o.status.success() {
            return Some(String::from_utf8_lossy(&o.stdout).into_owned());
        }
    }
    None
}

#[cfg(unix)]
fn apply(tag: &str) -> Result<String, String> {
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    let target = platform_target().ok_or("unsupported platform for self-update")?;
    let asset = format!("vudo-{tag}-{target}.tar.gz");
    let base = format!("https://github.com/{REPO}/releases/download/{tag}/{asset}");

    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe
        .parent()
        .ok_or("cannot locate the install directory")?
        .to_path_buf();

    let tmp = std::env::temp_dir().join(format!("vudo-update-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let _guard = TmpGuard(tmp.clone());

    let tgz = tmp.join(&asset);
    download(&base, &tgz)?;
    let shafile = tmp.join(format!("{asset}.sha256"));
    download(&format!("{base}.sha256"), &shafile)?;
    verify_sha(&tgz, &shafile)?;

    run_ok("tar", &["xzf", path_str(&tgz)?, "-C", path_str(&tmp)?])?;
    let src = tmp.join(format!("vudo-{tag}-{target}")).join("vudo");
    if !src.exists() {
        return Err("extracted binary not found".to_string());
    }

    // Stage in the SAME directory as the current exe, then rename over it:
    // rename(2) is atomic and is permitted even though the binary is running
    // (a plain overwrite would fail with ETXTBSY / "text file busy").
    let staged = dir.join(format!(".vudo.update.{}", std::process::id()));
    std::fs::copy(&src, &staged).map_err(|e| format!("writing to {}: {e}", dir.display()))?;
    std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| e.to_string())?;
    std::fs::rename(&staged, &exe).map_err(|e| {
        let _ = std::fs::remove_file(&staged);
        format!("replacing {}: {e}", exe.display())
    })?;

    return Ok(exe.display().to_string());

    fn path_str(p: &Path) -> Result<&str, String> {
        p.to_str().ok_or_else(|| "non-UTF-8 path".to_string())
    }
}

#[cfg(windows)]
fn apply(tag: &str) -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe
        .parent()
        .ok_or("cannot locate the install directory")?
        .to_str()
        .ok_or("non-UTF-8 path")?
        .replace('\'', "''");

    // Reuse the PowerShell installer, pinned to this tag and targeting the
    // current install dir. It moves the running exe aside and copies the new
    // one in (a running .exe can be renamed but not overwritten on Windows).
    let command = format!(
        "$env:VUDO_INSTALL_DIR = '{dir}'; $env:VUDO_VERSION = '{}'; \
         irm https://raw.githubusercontent.com/{REPO}/main/install.ps1 | iex",
        tag.replace('\'', "''")
    );
    let status = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &command,
        ])
        .status()
        .map_err(|e| format!("powershell: {e}"))?;
    if status.success() {
        Ok(exe.display().to_string())
    } else {
        Err("the PowerShell installer reported an error".to_string())
    }
}

#[cfg(unix)]
fn platform_target() -> Option<String> {
    let t = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "x86_64-unknown-linux-musl",
        ("linux", "aarch64") => "aarch64-unknown-linux-musl",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        _ => return None,
    };
    Some(t.to_string())
}

#[cfg(unix)]
fn download(url: &str, dest: &std::path::Path) -> Result<(), String> {
    let d = dest.to_str().ok_or("non-UTF-8 path")?;
    if run_ok("curl", &["-fsSL", "-o", d, url]).is_ok() {
        return Ok(());
    }
    if run_ok("wget", &["-qO", d, url]).is_ok() {
        return Ok(());
    }
    Err(format!("could not download {url} (need curl or wget)"))
}

#[cfg(unix)]
fn verify_sha(file: &std::path::Path, shafile: &std::path::Path) -> Result<(), String> {
    let expected = std::fs::read_to_string(shafile)
        .map_err(|e| e.to_string())?
        .split_whitespace()
        .next()
        .ok_or("empty checksum file")?
        .to_string();
    let actual = sha256(file)?;
    if expected != actual {
        return Err(format!(
            "checksum mismatch (expected {expected}, got {actual})"
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn sha256(file: &std::path::Path) -> Result<String, String> {
    use std::process::Command;
    let f = file.to_str().ok_or("non-UTF-8 path")?;
    for (bin, args) in [("sha256sum", vec![f]), ("shasum", vec!["-a", "256", f])] {
        if let Ok(o) = Command::new(bin).args(&args).output() {
            if o.status.success() {
                if let Some(tok) = String::from_utf8_lossy(&o.stdout).split_whitespace().next() {
                    return Ok(tok.to_string());
                }
            }
        }
    }
    Err("no sha256 tool found (sha256sum or shasum)".to_string())
}

#[cfg(unix)]
fn run_ok(program: &str, args: &[&str]) -> Result<(), String> {
    match std::process::Command::new(program).args(args).status() {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(format!("{program} exited with {s}")),
        Err(e) => Err(format!("{program}: {e}")),
    }
}

#[cfg(unix)]
struct TmpGuard(std::path::PathBuf);

#[cfg(unix)]
impl Drop for TmpGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::parse_tag;

    #[test]
    fn parses_tag_name() {
        let body = r#"{"url":"x","tag_name": "v1.2.3","name":"v1.2.3"}"#;
        assert_eq!(parse_tag(body).as_deref(), Some("v1.2.3"));
    }

    #[test]
    fn missing_tag_is_none() {
        assert_eq!(parse_tag(r#"{"message":"Not Found"}"#), None);
    }

    #[test]
    fn version_comparison() {
        use super::is_newer;
        assert!(is_newer("0.2.0", "0.3.0"));
        assert!(is_newer("0.3.0", "0.3.1"));
        assert!(is_newer("0.3.0", "1.0.0"));
        assert!(!is_newer("0.3.0", "0.3.0")); // equal
        assert!(!is_newer("0.3.0", "0.2.0")); // older release, don't downgrade
        assert!(!is_newer("0.3.0", "0.2.9"));
    }
}

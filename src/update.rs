use std::error::Error;
use std::io::{self, IsTerminal, Write};
use std::path::Path;
use std::process::Command;

use semver::Version;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallType {
    Homebrew,
    GitHub,
}

/// Install type from path and optional Homebrew prefix.
pub fn install_type_from_path(exe_path: &Path, brew_prefix: Option<&Path>) -> InstallType {
    let path_str = exe_path.to_string_lossy();
    if path_str.contains("Cellar") {
        return InstallType::Homebrew;
    }
    if let Some(prefix) = brew_prefix {
        let canonical_exe = exe_path.canonicalize().unwrap_or_else(|_| exe_path.to_path_buf());
        let canonical_prefix = prefix.canonicalize().unwrap_or_else(|_| prefix.to_path_buf());
        if canonical_exe.starts_with(&canonical_prefix) {
            return InstallType::Homebrew;
        }
    }
    InstallType::GitHub
}

/// Detects install type (exe path and brew --prefix).
pub fn current_install_type(exe_path: &Path) -> InstallType {
    let canonical = exe_path.canonicalize().unwrap_or_else(|_| exe_path.to_path_buf());
    let brew_prefix = Command::new("brew")
        .arg("--prefix")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| {
            let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if s.is_empty() {
                None
            } else {
                Some(std::path::PathBuf::from(s))
            }
        });
    install_type_from_path(&canonical, brew_prefix.as_deref())
}

/// Release target triple for current platform, or None if unsupported.
pub fn release_target() -> Option<&'static str> {
    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    match (os, arch) {
        ("macos", "aarch64") => Some("aarch64-apple-darwin"),
        ("macos", "x86_64") => Some("x86_64-apple-darwin"),
        ("linux", "aarch64") => Some("aarch64-unknown-linux-gnu"),
        ("linux", "x86_64") => Some("x86_64-unknown-linux-gnu"),
        ("windows", "x86_64") => Some("x86_64-pc-windows-msvc"),
        _ => None,
    }
}

fn strip_v(s: &str) -> &str {
    s.strip_prefix('v').unwrap_or(s).trim()
}

/// True if latest is newer than current (semver).
pub fn is_newer(current: &str, latest: &str) -> bool {
    let cur = Version::parse(strip_v(current)).ok();
    let lat = Version::parse(strip_v(latest)).ok();
    match (cur, lat) {
        (Some(c), Some(l)) => l > c,
        _ => false,
    }
}

const GITHUB_RELEASES_URL: &str = "https://api.github.com/repos/drugoi/diddo-hooks/releases/latest";

/// Fetches latest release tag from GitHub (no leading 'v').
pub fn fetch_latest_release_tag() -> Result<String, Box<dyn Error>> {
    let client = reqwest::blocking::Client::builder()
        .user_agent(format!("diddo/{}", env!("CARGO_PKG_VERSION")))
        .build()?;
    let resp = client.get(GITHUB_RELEASES_URL).send()?;
    let json: serde_json::Value = resp.json()?;
    let tag = json
        .get("tag_name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| std::io::Error::other("missing tag_name in release"))?;
    Ok(strip_v(tag).to_string())
}

/// Confirm with user; true to proceed. No prompt if assume_yes or non-TTY.
pub fn confirm_update(current: &str, latest: &str, assume_yes: bool) -> bool {
    if assume_yes {
        return true;
    }
    if !io::stdin().is_terminal() {
        eprintln!("A new version is available. Run with --yes to update non-interactively.");
        return false;
    }
    print!("Update diddo {current} → {latest}? [y/N] ");
    let _ = io::stdout().flush();
    let mut line = String::new();
    if io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    matches!(line.trim().to_lowercase().chars().next(), Some('y'))
}

pub fn run(assume_yes: bool) -> Result<(), Box<dyn Error>> {
    let current = env!("CARGO_PKG_VERSION");
    let latest = match fetch_latest_release_tag() {
        Ok(tag) => tag,
        Err(e) => return Err(format!("Could not check for updates: {e}").into()),
    };
    if !is_newer(current, &latest) {
        println!("diddo is already up to date ({current}).");
        return Ok(());
    }
    let exe = std::env::current_exe()?;
    let install_type = current_install_type(&exe);
    let target = release_target();

    if install_type == InstallType::Homebrew {
        if Command::new("brew").arg("--version").output().is_err() {
            return Err("Homebrew update requested but `brew` not found.".into());
        }
        if !confirm_update(current, &latest, assume_yes) {
            return Ok(());
        }
        let status = Command::new("brew").args(["upgrade", "diddo"]).status()?;
        if !status.success() {
            return Err("Update failed: brew upgrade diddo failed.".into());
        }
        println!("Updated to {latest}.");
        return Ok(());
    }

    let target = match target {
        Some(t) => t,
        None => return Err("No release available for your platform (unsupported target).".into()),
    };
    if !confirm_update(current, &latest, assume_yes) {
        return Ok(());
    }
    let result = self_update::backends::github::Update::configure()
        .repo_owner("drugoi")
        .repo_name("diddo-hooks")
        .bin_name("diddo")
        .current_version(current)
        .target(target)
        .target_version_tag(&format!("v{latest}"))
        .no_confirm(true)
        .show_download_progress(true)
        .build()
        .map_err(|e| format!("Could not configure update: {e}"))?
        .update()
        .map_err(|e| {
            format!(
                "Update failed: could not replace binary ({e}). \
                 You can download the new version from https://github.com/drugoi/diddo-hooks/releases."
            )
        })?;
    if let self_update::Status::Updated(ver) = result {
        println!("Updated to {ver}.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{install_type_from_path, is_newer, release_target, InstallType};
    use std::path::Path;

    #[test]
    fn install_type_homebrew_when_path_contains_cellar() {
        let path = Path::new("/opt/homebrew/Cellar/diddo/0.5.0/bin/diddo");
        assert_eq!(
            install_type_from_path(path, None),
            InstallType::Homebrew
        );
    }

    #[test]
    fn install_type_homebrew_when_path_under_prefix() {
        let path = Path::new("/opt/homebrew/bin/diddo");
        let prefix = Path::new("/opt/homebrew");
        assert_eq!(
            install_type_from_path(path, Some(prefix)),
            InstallType::Homebrew
        );
    }

    #[test]
    fn install_type_github_when_path_not_homebrew() {
        let path = Path::new("/usr/local/bin/diddo");
        assert_eq!(install_type_from_path(path, None), InstallType::GitHub);
    }

    #[test]
    fn install_type_github_when_path_not_under_given_prefix() {
        let path = Path::new("/usr/local/bin/diddo");
        let prefix = Path::new("/opt/homebrew");
        assert_eq!(
            install_type_from_path(path, Some(prefix)),
            InstallType::GitHub
        );
    }

    #[test]
    fn release_target_returns_some_for_supported_platform() {
        let target = release_target();
        assert!(target.is_some(), "release_target should be Some on supported platform");
        let t = target.unwrap();
        assert!(
            t.contains("darwin") || t.contains("linux") || t.contains("windows"),
            "target should be a known triple: {}",
            t
        );
    }

    #[test]
    fn is_newer_returns_true_when_latest_greater() {
        assert!(is_newer("0.5.0", "0.6.0"));
    }

    #[test]
    fn is_newer_returns_false_when_same() {
        assert!(!is_newer("0.5.0", "0.5.0"));
    }

    #[test]
    fn is_newer_returns_false_when_current_greater() {
        assert!(!is_newer("0.6.0", "0.5.0"));
    }

    #[test]
    fn is_newer_strips_v_prefix() {
        assert!(is_newer("0.5.0", "v0.6.0"));
    }
}

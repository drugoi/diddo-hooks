use std::error::Error;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallType {
    Homebrew,
    GitHub,
}

/// Determines install type from the executable path and optional Homebrew prefix.
/// Used for testing without calling `brew --prefix`; production code uses `current_install_type`.
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

/// Detects install type by resolving the exe path and optionally running `brew --prefix`.
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

/// Returns the release target triple for the current platform, or None if unsupported.
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

pub fn run(assume_yes: bool) -> Result<(), Box<dyn Error>> {
    let exe = std::env::current_exe()?;
    let _install_type = current_install_type(&exe);
    let _target = release_target();
    let _ = assume_yes;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{install_type_from_path, release_target, InstallType};
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
}

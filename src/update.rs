use std::error::Error;
use std::io::{self, IsTerminal, Write};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use semver::Version;
use serde::{Deserialize, Serialize};

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
        let canonical_exe = exe_path
            .canonicalize()
            .unwrap_or_else(|_| exe_path.to_path_buf());
        let canonical_prefix = prefix
            .canonicalize()
            .unwrap_or_else(|_| prefix.to_path_buf());
        if canonical_exe.starts_with(&canonical_prefix) {
            return InstallType::Homebrew;
        }
    }
    InstallType::GitHub
}

/// Detects install type (exe path and brew --prefix).
pub fn current_install_type(exe_path: &Path) -> InstallType {
    let canonical = exe_path
        .canonicalize()
        .unwrap_or_else(|_| exe_path.to_path_buf());
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
    s.trim_start_matches('v').trim()
}

/// Compares two version strings. Returns `None` if either side fails to
/// parse as semver, `Some(true)` if `latest` is newer than `current`.
fn compare_versions(current: &str, latest: &str) -> Option<bool> {
    let cur = Version::parse(strip_v(current)).ok();
    let lat = Version::parse(strip_v(latest)).ok();
    match (cur, lat) {
        (Some(c), Some(l)) => Some(l > c),
        _ => None,
    }
}

/// True if latest is newer than current (semver). Unparseable input is
/// treated as "not newer" — see `compare_versions` for a version that
/// distinguishes "not newer" from "could not tell".
pub fn is_newer(current: &str, latest: &str) -> bool {
    compare_versions(current, latest).unwrap_or(false)
}

const GITHUB_RELEASES_URL: &str = "https://api.github.com/repos/drugoi/diddo-hooks/releases/latest";

/// Fetches latest release tag from GitHub (no leading 'v').
pub fn fetch_latest_release_tag() -> Result<String, Box<dyn Error>> {
    let client = reqwest::blocking::Client::builder()
        .user_agent(format!("diddo/{}", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(5))
        .build()?;
    let resp = client.get(GITHUB_RELEASES_URL).send()?;
    let json: serde_json::Value = resp.json()?;
    let tag = json
        .get("tag_name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| std::io::Error::other("missing tag_name in release"))?;
    Ok(strip_v(tag).to_string())
}

const CACHE_TTL_SECS: i64 = 2 * 60 * 60; // 2 hours

#[derive(Debug, Serialize, Deserialize)]
struct UpdateCache {
    latest_version: String,
    checked_at: i64,
}

/// Write the cache atomically (temp file + rename) so a killed thread can't
/// leave a corrupt half-written file.
fn write_cache(cache_path: &Path, cache: &UpdateCache) {
    if let Some(parent) = cache_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string(cache) {
        let tmp = cache_path.with_extension("json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, cache_path);
        }
    }
}

/// Check for a newer version, using a file cache to avoid hitting GitHub too often.
/// Returns `Some(latest_version)` if a newer version is available, `None` otherwise.
/// Any error is silently swallowed.
pub fn check_for_update(cache_path: &Path) -> Option<String> {
    check_for_update_with(cache_path, fetch_latest_release_tag)
}

fn check_for_update_with<F>(cache_path: &Path, fetch: F) -> Option<String>
where
    F: FnOnce() -> Result<String, Box<dyn Error>>,
{
    let current = env!("CARGO_PKG_VERSION");

    // Try reading cached result
    if let Ok(contents) = std::fs::read_to_string(cache_path)
        && let Ok(cache) = serde_json::from_str::<UpdateCache>(&contents)
    {
        let now = chrono::Utc::now().timestamp();
        if now - cache.checked_at < CACHE_TTL_SECS {
            return if is_newer(current, &cache.latest_version) {
                Some(cache.latest_version)
            } else {
                None
            };
        }
    }

    // Cache miss or stale — write a throttle record FIRST so a killed process
    // (e.g. main exits before the fetch below returns) cannot retry before TTL.
    write_cache(
        cache_path,
        &UpdateCache {
            latest_version: current.to_string(),
            checked_at: chrono::Utc::now().timestamp(),
        },
    );

    // Now perform the fetch. On error, leave the throttle record in place and
    // return None (this replaces the old error-path negative-cache write —
    // same effect, now crash-safe).
    let latest = match fetch() {
        Ok(tag) => tag,
        Err(_) => return None,
    };

    write_cache(
        cache_path,
        &UpdateCache {
            latest_version: latest.clone(),
            checked_at: chrono::Utc::now().timestamp(),
        },
    );

    if is_newer(current, &latest) {
        Some(latest)
    } else {
        None
    }
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
    match compare_versions(current, &latest) {
        None => {
            eprintln!(
                "warning: could not compare versions: release tag '{latest}' is not valid semver"
            );
            return Err("Could not check for updates: release tag is not valid semver".into());
        }
        Some(false) => {
            println!("diddo is already up to date ({current}).");
            return Ok(());
        }
        Some(true) => {}
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
    use super::{InstallType, install_type_from_path, is_newer, release_target};
    use std::path::Path;

    #[test]
    fn install_type_homebrew_when_path_contains_cellar() {
        let path = Path::new("/opt/homebrew/Cellar/diddo/0.5.0/bin/diddo");
        assert_eq!(install_type_from_path(path, None), InstallType::Homebrew);
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
        assert!(
            target.is_some(),
            "release_target should be Some on supported platform"
        );
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

    #[test]
    fn strip_v_strips_repeated_prefixes() {
        assert_eq!(super::strip_v("vv0.4.0"), "0.4.0");
    }

    #[test]
    fn compare_versions_returns_none_for_unparseable_latest() {
        assert_eq!(super::compare_versions("0.6.7", "not-a-version"), None);
    }

    #[test]
    fn check_for_update_returns_none_when_cache_has_current_version() {
        use super::{UpdateCache, check_for_update};

        let dir = std::env::temp_dir().join(format!("diddo-update-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cache_path = dir.join("update_check.json");

        let cache = UpdateCache {
            latest_version: env!("CARGO_PKG_VERSION").to_string(),
            checked_at: chrono::Utc::now().timestamp(),
        };
        std::fs::write(&cache_path, serde_json::to_string(&cache).unwrap()).unwrap();

        assert_eq!(check_for_update(&cache_path), None);

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn check_for_update_returns_some_when_cache_has_newer_version() {
        use super::{UpdateCache, check_for_update};

        let dir =
            std::env::temp_dir().join(format!("diddo-update-test-newer-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cache_path = dir.join("update_check.json");

        let cache = UpdateCache {
            latest_version: "99.99.99".to_string(),
            checked_at: chrono::Utc::now().timestamp(),
        };
        std::fs::write(&cache_path, serde_json::to_string(&cache).unwrap()).unwrap();

        assert_eq!(check_for_update(&cache_path), Some("99.99.99".to_string()));

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn stale_cache_triggers_fetch_and_stores_result() {
        use super::{UpdateCache, check_for_update_with};

        let dir =
            std::env::temp_dir().join(format!("diddo-update-test-stale-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cache_path = dir.join("update_check.json");

        // Cache is older than TTL (2h) and reports an old version.
        let stale = UpdateCache {
            latest_version: "0.0.1".to_string(),
            checked_at: chrono::Utc::now().timestamp() - super::CACHE_TTL_SECS - 1,
        };
        std::fs::write(&cache_path, serde_json::to_string(&stale).unwrap()).unwrap();

        let result = check_for_update_with(&cache_path, || Ok("9.9.9".to_string()));
        assert_eq!(result, Some("9.9.9".to_string()));

        let contents = std::fs::read_to_string(&cache_path).unwrap();
        let saved: UpdateCache = serde_json::from_str(&contents).unwrap();
        assert_eq!(saved.latest_version, "9.9.9");

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn throttle_record_written_before_fetch() {
        use super::{UpdateCache, check_for_update_with};
        use std::sync::{Arc, Mutex};

        let dir = std::env::temp_dir().join(format!(
            "diddo-update-test-throttle-before-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let cache_path = dir.join("update_check.json");
        // No pre-existing cache: forces the fetch path.

        let observed: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let observed_clone = Arc::clone(&observed);
        let cache_path_clone = cache_path.clone();

        let result = check_for_update_with(&cache_path, move || {
            // Read the cache file HERE, inside the closure, i.e. before the
            // fetch "returns" — this proves the throttle record is written
            // before the fetch happens.
            let contents = std::fs::read_to_string(&cache_path_clone).unwrap();
            let cache: UpdateCache = serde_json::from_str(&contents).unwrap();
            *observed_clone.lock().unwrap() = Some(cache.latest_version);
            Err("net down".into())
        });

        assert_eq!(result, None);
        assert_eq!(
            observed.lock().unwrap().as_deref(),
            Some(env!("CARGO_PKG_VERSION"))
        );

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn fetch_error_leaves_throttle_record() {
        use super::check_for_update_with;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let dir = std::env::temp_dir().join(format!(
            "diddo-update-test-throttle-after-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let cache_path = dir.join("update_check.json");

        // First call: fetch fails, but the pre-written throttle record stays.
        let first = check_for_update_with(&cache_path, || Err("net down".into()));
        assert_eq!(first, None);

        // Second immediate call: the fresh throttle record must suppress any
        // further fetch attempt.
        let was_called = Arc::new(AtomicBool::new(false));
        let was_called_clone = Arc::clone(&was_called);
        let second = check_for_update_with(&cache_path, move || {
            was_called_clone.store(true, Ordering::SeqCst);
            Err("net down".into())
        });

        assert_eq!(second, None);
        assert!(!was_called.load(Ordering::SeqCst));

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cache_write_is_atomic() {
        use super::check_for_update_with;

        let dir =
            std::env::temp_dir().join(format!("diddo-update-test-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cache_path = dir.join("update_check.json");
        let tmp_path = cache_path.with_extension("json.tmp");

        let result = check_for_update_with(&cache_path, || Ok("9.9.9".to_string()));
        assert_eq!(result, Some("9.9.9".to_string()));
        assert!(!tmp_path.exists(), "no .json.tmp file should remain");

        std::fs::remove_dir_all(dir).unwrap();
    }
}

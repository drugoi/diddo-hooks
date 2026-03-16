use std::{
    fs, io,
    path::{Path, PathBuf},
    process::Command,
};

use crate::paths::AppPaths;

#[derive(Debug)]
pub struct HooksStatus {
    pub global: GlobalHookStatus,
    pub local: LocalHookStatus,
}

#[derive(Debug)]
pub enum GlobalHookStatus {
    ManagedByDiddo(String),
    Other(String),
    NotSet,
}

#[derive(Debug)]
pub enum LocalHookStatus {
    DiddoInstalled(String),
    NoDiddoHook(String),
    NotSet,
    NotInRepo,
}

const POST_COMMIT_FILE: &str = "post-commit";
const POST_COMMIT_DIDDO_PREV: &str = "post-commit.diddo-prev";
const DIDDO_MANAGED_MARKER: &str = "# diddo-managed";
const STATE_FILE: &str = "diddo-managed-state";
const HOOK_NAMES: &[&str] = &[
    "applypatch-msg",
    "pre-applypatch",
    "post-applypatch",
    "pre-commit",
    "pre-merge-commit",
    "prepare-commit-msg",
    "commit-msg",
    "post-commit",
    "pre-rebase",
    "post-checkout",
    "post-merge",
    "pre-push",
    "pre-receive",
    "update",
    "proc-receive",
    "post-receive",
    "post-update",
    "reference-transaction",
    "push-to-checkout",
    "pre-auto-gc",
    "post-rewrite",
    "sendemail-validate",
];

#[derive(Debug, Clone, PartialEq, Eq)]
struct HookPathState {
    raw: String,
    resolved: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum UninstallOutcome {
    RestoredPrevious(String),
    UnsetManaged,
    LeftCurrentUntouched(Option<String>),
}

pub fn install(paths: &AppPaths) -> io::Result<()> {
    install_with(paths, get_global_hooks_dir, set_global_hooks_dir)?;
    println!(
        "Installed diddo hooks in {} and updated global core.hooksPath.",
        paths.hooks_dir.display()
    );

    if let Ok(Some((local_raw, local_resolved))) = get_local_hooks_dir()
        && !same_path(&local_resolved, &paths.hooks_dir)
    {
        install_local_hook(paths, &local_resolved, &local_raw)?;
        println!(
            "Also added diddo post-commit to this repo's local hooks ({}) so commits are recorded.",
            local_raw
        );
    }

    Ok(())
}

pub fn uninstall() -> io::Result<()> {
    let paths = AppPaths::new()?;
    let outcome = uninstall_with(
        &paths,
        get_global_hooks_dir,
        restore_global_hooks_dir,
        unset_global_hooks_dir,
    )?;

    match outcome {
        UninstallOutcome::RestoredPrevious(previous) => {
            println!("Removed diddo hooks and restored global core.hooksPath to {previous}.");
        }
        UninstallOutcome::UnsetManaged => {
            println!("Removed diddo hooks and unset global core.hooksPath.");
        }
        UninstallOutcome::LeftCurrentUntouched(Some(current)) => {
            println!(
                "Removed diddo hooks without changing global core.hooksPath because it now points to {current}."
            );
        }
        UninstallOutcome::LeftCurrentUntouched(None) => {
            println!(
                "Removed diddo hooks without changing global core.hooksPath because it no longer points to the managed hooks directory."
            );
        }
    }

    Ok(())
}

pub fn hooks_status(paths: &AppPaths) -> io::Result<HooksStatus> {
    let global = match get_global_hooks_dir()? {
        Some(state) => {
            if same_path(&state.resolved, &paths.hooks_dir) {
                GlobalHookStatus::ManagedByDiddo(state.raw)
            } else {
                GlobalHookStatus::Other(state.raw)
            }
        }
        None => GlobalHookStatus::NotSet,
    };

    let local = match get_local_hooks_dir() {
        Ok(Some((raw, resolved))) => {
            let post_commit = resolved.join(POST_COMMIT_FILE);
            if post_commit.exists() {
                let contents = fs::read_to_string(&post_commit).unwrap_or_default();
                if contents.contains(DIDDO_MANAGED_MARKER) {
                    LocalHookStatus::DiddoInstalled(raw)
                } else {
                    LocalHookStatus::NoDiddoHook(raw)
                }
            } else {
                LocalHookStatus::NoDiddoHook(raw)
            }
        }
        Ok(None) => match get_repo_root() {
            Ok(Some(_)) => LocalHookStatus::NotSet,
            _ => LocalHookStatus::NotInRepo,
        },
        Err(_) => LocalHookStatus::NotInRepo,
    };

    Ok(HooksStatus { global, local })
}

fn build_post_commit_script(previous_hooks_path: Option<&str>) -> String {
    let mut script = String::from("#!/bin/sh\nset -u\n");
    script.push_str(DIDDO_MANAGED_MARKER);
    script.push_str("\n\ndiddo_status=0\n");

    match resolve_diddo_executable() {
        Ok(Some(diddo_path)) => {
            script.push_str(&format!(
                "diddo_path={}\nif [ -x \"$diddo_path\" ]; then\n  \"$diddo_path\" hook || diddo_status=$?\nelse\n  diddo hook || diddo_status=$?\nfi\n",
                shell_single_quote(&path_for_script(&diddo_path))
            ));
        }
        _ => script.push_str("diddo hook || diddo_status=$?\n"),
    }

    if let Some(previous_hooks_path) = previous_hooks_path {
        script.push('\n');
        script.push_str("previous_status=0\n");
        script.push_str(&build_previous_hook_invocation(
            previous_hooks_path,
            POST_COMMIT_FILE,
            "previous_status",
        ));
        script.push_str(
            "\nif [ \"$previous_status\" -ne 0 ]; then\n  exit \"$previous_status\"\nfi\n",
        );
    }

    script.push_str("\nif [ \"$diddo_status\" -ne 0 ]; then\n  exit \"$diddo_status\"\nfi\n");

    script
}

fn build_forwarding_hook_script(previous_hooks_path: &str, hook_name: &str) -> String {
    format!(
        "#!/bin/sh\nset -eu\n\n{}if [ -x \"$previous_hook_path\" ]; then\n  \"$previous_hook_path\" \"$@\"\nfi\n",
        build_previous_hook_path_resolution(previous_hooks_path, hook_name)
    )
}

fn build_previous_hook_invocation(
    previous_hooks_path: &str,
    hook_name: &str,
    status_var: &str,
) -> String {
    format!(
        "{}if [ -x \"$previous_hook_path\" ]; then\n  \"$previous_hook_path\" \"$@\" || {status_var}=$?\nfi\n",
        build_previous_hook_path_resolution(previous_hooks_path, hook_name)
    )
}

fn build_previous_hook_path_resolution(previous_hooks_path: &str, hook_name: &str) -> String {
    let quoted_hooks_path = shell_single_quote(previous_hooks_path);
    let quoted_hook_name = shell_single_quote(hook_name);

    format!(
        "previous_hooks_path={quoted_hooks_path}\nprevious_hook_name={quoted_hook_name}\ncase \"$previous_hooks_path\" in\n  /*|[A-Za-z]:/*|//*)\n    previous_hook_path=\"$previous_hooks_path/$previous_hook_name\"\n    ;;\n  [A-Za-z]:\\\\*|\\\\\\\\*)\n    previous_hook_path=\"$previous_hooks_path\\\\$previous_hook_name\"\n    ;;\n  \"~\")\n    previous_hook_path=\"$HOME/$previous_hook_name\"\n    ;;\n  \"~/\"*)\n    previous_hook_path=\"$HOME/${{previous_hooks_path#~/}}/$previous_hook_name\"\n    ;;\n  *)\n    previous_hook_path=\"$PWD/$previous_hooks_path/$previous_hook_name\"\n    ;;\nesac\n"
    )
}

fn install_with<FGet, FSet>(
    paths: &AppPaths,
    mut get_existing_hooks_dir: FGet,
    mut set_managed_hooks_dir: FSet,
) -> io::Result<()>
where
    FGet: FnMut() -> io::Result<Option<HookPathState>>,
    FSet: FnMut(&Path) -> io::Result<()>,
{
    fs::create_dir_all(&paths.hooks_dir)?;

    let current_hooks_dir = get_existing_hooks_dir()?;
    let previous_hooks_dir = resolve_previous_hooks_dir_for_install(paths, current_hooks_dir)?;

    clear_managed_hooks_dir(&paths.hooks_dir)?;
    write_previous_hooks_state(
        &paths.hooks_dir,
        previous_hooks_dir.as_ref().map(|state| state.raw.as_str()),
    )?;

    if let Some(previous_hooks_dir) = previous_hooks_dir.as_ref() {
        create_forwarding_hooks(&previous_hooks_dir.raw, &paths.hooks_dir)?;
    }

    let generated_post_commit = paths.hooks_dir.join(POST_COMMIT_FILE);
    fs::write(
        &generated_post_commit,
        build_post_commit_script(previous_hooks_dir.as_ref().map(|state| state.raw.as_str())),
    )?;
    set_executable_if_unix(&generated_post_commit)?;

    set_managed_hooks_dir(&paths.hooks_dir)
}

fn uninstall_with<FGet, FRestore, FUnset>(
    paths: &AppPaths,
    mut get_current_hooks_dir: FGet,
    mut restore_previous_hooks_dir: FRestore,
    mut unset_managed_hooks_dir: FUnset,
) -> io::Result<UninstallOutcome>
where
    FGet: FnMut() -> io::Result<Option<HookPathState>>,
    FRestore: FnMut(&str) -> io::Result<()>,
    FUnset: FnMut() -> io::Result<()>,
{
    let previous_hooks_path = read_previous_hooks_state(&paths.hooks_dir)?;
    let current_hooks_dir = get_current_hooks_dir()?;
    let diddo_still_owns_config = current_hooks_dir
        .as_ref()
        .is_some_and(|state| same_path(&state.resolved, &paths.hooks_dir));

    let outcome = if diddo_still_owns_config {
        if let Some(previous_hooks_path) = previous_hooks_path.as_deref() {
            restore_previous_hooks_dir(previous_hooks_path)?;
            UninstallOutcome::RestoredPrevious(previous_hooks_path.to_string())
        } else {
            unset_managed_hooks_dir()?;
            UninstallOutcome::UnsetManaged
        }
    } else {
        UninstallOutcome::LeftCurrentUntouched(current_hooks_dir.map(|state| state.raw))
    };

    if paths.hooks_dir.exists() {
        fs::remove_dir_all(&paths.hooks_dir)?;
    }

    Ok(outcome)
}

fn resolve_previous_hooks_dir_for_install(
    paths: &AppPaths,
    current_hooks_dir: Option<HookPathState>,
) -> io::Result<Option<HookPathState>> {
    match current_hooks_dir {
        Some(current) if same_path(&current.resolved, &paths.hooks_dir) => {
            let previous_raw = read_previous_hooks_state(&paths.hooks_dir)?;
            Ok(previous_raw.map(|raw| HookPathState {
                resolved: PathBuf::from(&raw),
                raw,
            }))
        }
        other => Ok(other),
    }
}

fn clear_managed_hooks_dir(managed_hooks_dir: &Path) -> io::Result<()> {
    if !managed_hooks_dir.exists() {
        return Ok(());
    }

    for entry in fs::read_dir(managed_hooks_dir)? {
        let entry = entry?;
        let path = entry.path();

        if entry.file_type()?.is_dir() {
            fs::remove_dir_all(path)?;
        } else {
            fs::remove_file(path)?;
        }
    }

    Ok(())
}

fn write_previous_hooks_state(
    managed_hooks_dir: &Path,
    previous_hooks_path: Option<&str>,
) -> io::Result<()> {
    let state_path = managed_hooks_dir.join(STATE_FILE);
    let contents = previous_hooks_path
        .map(|path| format!("{path}\n"))
        .unwrap_or_default();
    fs::write(state_path, contents)
}

fn read_previous_hooks_state(managed_hooks_dir: &Path) -> io::Result<Option<String>> {
    let state_path = managed_hooks_dir.join(STATE_FILE);

    match fs::read_to_string(state_path) {
        Ok(contents) => {
            let raw = contents.trim().to_string();
            if raw.is_empty() {
                Ok(None)
            } else {
                Ok(Some(raw))
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn create_forwarding_hooks(previous_hooks_path: &str, managed_hooks_dir: &Path) -> io::Result<()> {
    for hook_name in HOOK_NAMES {
        if *hook_name == POST_COMMIT_FILE {
            continue;
        }

        let target_hook = managed_hooks_dir.join(hook_name);
        fs::write(
            &target_hook,
            build_forwarding_hook_script(previous_hooks_path, hook_name),
        )?;
        set_executable_if_unix(&target_hook)?;
    }

    Ok(())
}

fn same_path(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }

    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn get_global_hooks_dir() -> io::Result<Option<HookPathState>> {
    let output = Command::new("git")
        .args(["config", "--global", "--get", "core.hooksPath"])
        .output()?;

    if output.status.success() {
        let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();

        if raw.is_empty() {
            return Ok(None);
        }

        return Ok(Some(HookPathState {
            resolved: resolve_hooks_path_for_comparison(
                &raw,
                &std::env::current_dir()?,
                home_dir().as_deref(),
            )?,
            raw,
        }));
    }

    if output.status.code() == Some(1) {
        return Ok(None);
    }

    Err(git_config_error(
        &["config", "--global", "--get", "core.hooksPath"],
        &output.stderr,
    ))
}

fn get_repo_root() -> io::Result<Option<PathBuf>> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()?;

    if output.status.success() {
        let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if raw.is_empty() {
            return Ok(None);
        }
        return Ok(Some(PathBuf::from(raw)));
    }

    if output.status.code() == Some(128) {
        return Ok(None);
    }

    Err(git_config_error(
        &["rev-parse", "--show-toplevel"],
        &output.stderr,
    ))
}

fn get_local_hooks_dir() -> io::Result<Option<(String, PathBuf)>> {
    let repo_root = match get_repo_root()? {
        Some(r) => r,
        None => return Ok(None),
    };

    let output = Command::new("git")
        .args(["config", "--local", "--get", "core.hooksPath"])
        .output()?;

    if !output.status.success() {
        if output.status.code() == Some(1) {
            return Ok(None);
        }
        return Err(git_config_error(
            &["config", "--local", "--get", "core.hooksPath"],
            &output.stderr,
        ));
    }

    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if raw.is_empty() {
        return Ok(None);
    }

    let resolved = resolve_hooks_path_for_comparison(&raw, &repo_root, home_dir().as_deref())?;

    Ok(Some((raw, resolved)))
}

fn install_local_hook(
    _paths: &AppPaths,
    local_hooks_dir: &Path,
    _local_raw: &str,
) -> io::Result<()> {
    fs::create_dir_all(local_hooks_dir)?;

    let post_commit_path = local_hooks_dir.join(POST_COMMIT_FILE);
    let prev_path = local_hooks_dir.join(POST_COMMIT_DIDDO_PREV);

    let previous_hooks_path = if post_commit_path.exists() {
        let contents = fs::read_to_string(&post_commit_path).unwrap_or_default();
        if contents.contains(DIDDO_MANAGED_MARKER) {
            if prev_path.exists() {
                Some(local_hooks_dir.display().to_string())
            } else {
                None
            }
        } else {
            fs::rename(&post_commit_path, &prev_path)?;
            Some(local_hooks_dir.display().to_string())
        }
    } else {
        None
    };

    let script = if previous_hooks_path.is_some() {
        build_post_commit_script_with_previous(
            &local_hooks_dir.display().to_string(),
            POST_COMMIT_DIDDO_PREV,
        )
    } else {
        build_post_commit_script(None)
    };

    fs::write(&post_commit_path, script)?;
    set_executable_if_unix(&post_commit_path)?;

    Ok(())
}

fn build_post_commit_script_with_previous(path: &str, hook_name: &str) -> String {
    let mut script = String::from("#!/bin/sh\nset -u\n");
    script.push_str(DIDDO_MANAGED_MARKER);
    script.push_str("\n\ndiddo_status=0\n");

    match resolve_diddo_executable() {
        Ok(Some(diddo_path)) => {
            script.push_str(&format!(
                "diddo_path={}\nif [ -x \"$diddo_path\" ]; then\n  \"$diddo_path\" hook || diddo_status=$?\nelse\n  diddo hook || diddo_status=$?\nfi\n",
                shell_single_quote(&path_for_script(&diddo_path))
            ));
        }
        _ => script.push_str("diddo hook || diddo_status=$?\n"),
    }

    script.push('\n');
    script.push_str("previous_status=0\n");
    script.push_str(&build_previous_hook_invocation(
        path,
        hook_name,
        "previous_status",
    ));
    script.push_str("\nif [ \"$previous_status\" -ne 0 ]; then\n  exit \"$previous_status\"\nfi\n");
    script.push_str("\nif [ \"$diddo_status\" -ne 0 ]; then\n  exit \"$diddo_status\"\nfi\n");

    script
}

fn set_global_hooks_dir(path: &Path) -> io::Result<()> {
    let output = Command::new("git")
        .args(["config", "--global", "core.hooksPath"])
        .arg(path)
        .output()?;

    if output.status.success() {
        return Ok(());
    }

    Err(git_config_error(
        &["config", "--global", "core.hooksPath"],
        &output.stderr,
    ))
}

fn restore_global_hooks_dir(raw_path: &str) -> io::Result<()> {
    let output = Command::new("git")
        .args(["config", "--global", "core.hooksPath", raw_path])
        .output()?;

    if output.status.success() {
        return Ok(());
    }

    Err(git_config_error(
        &["config", "--global", "core.hooksPath"],
        &output.stderr,
    ))
}

fn unset_global_hooks_dir() -> io::Result<()> {
    let output = Command::new("git")
        .args(["config", "--global", "--unset", "core.hooksPath"])
        .output()?;

    if output.status.success() || output.status.code() == Some(5) {
        return Ok(());
    }

    if output.status.code() == Some(1) {
        let stderr = String::from_utf8_lossy(&output.stderr);

        if stderr.contains("No such section or key") || stderr.trim().is_empty() {
            return Ok(());
        }
    }

    Err(git_config_error(
        &["config", "--global", "--unset", "core.hooksPath"],
        &output.stderr,
    ))
}

fn git_config_error(args: &[&str], stderr: &[u8]) -> io::Error {
    let stderr = String::from_utf8_lossy(stderr).trim().to_string();
    let command = format!("git {}", args.join(" "));
    let message = if stderr.is_empty() {
        format!("{command} failed")
    } else {
        format!("{command} failed: {stderr}")
    };

    io::Error::other(message)
}

fn resolve_hooks_path_for_comparison(
    raw_path: &str,
    repo_context: &Path,
    home_dir: Option<&Path>,
) -> io::Result<PathBuf> {
    let path = if raw_path == "~" {
        home_dir
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(raw_path))
    } else if let Some(relative_to_home) = raw_path.strip_prefix("~/") {
        home_dir
            .map(|home_dir| home_dir.join(relative_to_home))
            .unwrap_or_else(|| PathBuf::from(raw_path))
    } else {
        let path = PathBuf::from(raw_path);

        if path.is_absolute() || is_windows_absolute_path(raw_path) {
            path
        } else {
            repo_context.join(path)
        }
    };

    Ok(fs::canonicalize(&path).unwrap_or(path))
}

fn is_windows_absolute_path(raw_path: &str) -> bool {
    let bytes = raw_path.as_bytes();

    let has_drive_prefix = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\');

    has_drive_prefix || raw_path.starts_with("//") || raw_path.starts_with("\\\\")
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

/// Path string for embedding in shell scripts. On Windows, uses forward slashes so sh/Git Bash
/// sees a single path; on Unix unchanged.
fn path_for_script(path: &Path) -> std::borrow::Cow<'_, str> {
    let s = path.to_string_lossy();
    #[cfg(windows)]
    {
        std::borrow::Cow::Owned(s.replace('\\', "/"))
    }
    #[cfg(not(windows))]
    {
        s
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
}

fn resolve_diddo_executable() -> io::Result<Option<PathBuf>> {
    match std::env::current_exe() {
        Ok(path) => Ok(Some(fs::canonicalize(&path).unwrap_or(path))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn set_executable_if_unix(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions)?;
    }

    #[cfg(not(unix))]
    {
        let _ = path;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        sync::{Arc, Mutex},
        time::{SystemTime, UNIX_EPOCH},
    };

    use std::process::Command;

    use super::{
        HookPathState, POST_COMMIT_DIDDO_PREV, STATE_FILE, build_forwarding_hook_script,
        build_post_commit_script, install_local_hook, install_with, path_for_script,
        resolve_diddo_executable, resolve_hooks_path_for_comparison, uninstall_with,
    };
    use crate::paths::AppPaths;

    #[test]
    fn build_post_commit_script_runs_diddo_hook_without_chain_by_default() {
        let script = build_post_commit_script(None);

        assert!(script.contains("diddo hook"));
        assert!(!script.contains("previous_hooks_path="));
    }

    #[test]
    fn build_post_commit_script_prefers_resolved_diddo_executable_with_path_fallback() {
        let script = build_post_commit_script(None);
        let diddo_path = resolve_diddo_executable()
            .unwrap()
            .expect("current executable should be resolvable in tests");
        let path_in_script = path_for_script(&diddo_path);

        assert!(script.contains(&format!("diddo_path='{}'", path_in_script)));
        assert!(script.contains("if [ -x \"$diddo_path\" ]; then"));
        assert!(script.contains("\"$diddo_path\" hook || diddo_status=$?"));
        assert!(script.contains("else\n  diddo hook || diddo_status=$?\nfi"));
    }

    #[test]
    fn build_post_commit_script_chains_a_previous_post_commit_hook() {
        let script = build_post_commit_script(Some("/tmp/previous-hooks"));

        assert!(script.contains("diddo hook"));
        assert!(script.contains("previous_hooks_path='/tmp/previous-hooks'"));
        assert!(script.contains("previous_hook_name='post-commit'"));
        assert!(script.contains("if [ -x \"$previous_hook_path\" ]; then"));
        assert!(!script.contains("sh \"$previous_hook_path\""));
    }

    #[test]
    fn build_post_commit_script_runs_previous_hook_even_if_diddo_hook_fails() {
        let script = build_post_commit_script(Some("/tmp/previous-hooks"));

        assert!(script.contains("set -u"));
        assert!(!script.contains("set -eu"));
        assert!(script.contains("diddo_status=0"));
        assert!(script.contains("diddo hook || diddo_status=$?"));
        assert!(script.contains("previous_status=0"));
        assert!(script.contains("\"$previous_hook_path\" \"$@\" || previous_status=$?"));
        assert!(script.contains("if [ \"$previous_status\" -ne 0 ]; then"));
        assert!(script.contains("if [ \"$diddo_status\" -ne 0 ]; then"));
    }

    #[test]
    fn forwarding_wrapper_only_runs_executable_targets() {
        let script = build_forwarding_hook_script("/tmp/previous-hooks", "pre-commit");

        assert!(script.contains("if [ -x \"$previous_hook_path\" ]; then"));
        assert!(!script.contains("elif [ -f"));
        assert!(!script.contains("sh \"$previous_hook_path\""));
    }

    #[test]
    fn forwarding_wrapper_preserves_relative_hooks_path_for_runtime_resolution() {
        let script = build_forwarding_hook_script(".githooks", "pre-commit");

        assert!(script.contains("previous_hooks_path='.githooks'"));
        assert!(script.contains("previous_hook_name='pre-commit'"));
        assert!(
            script.contains("previous_hook_path=\"$PWD/$previous_hooks_path/$previous_hook_name\"")
        );
        assert!(!script.contains("/tmp"));
    }

    #[test]
    fn forwarding_wrapper_recognizes_windows_drive_absolute_paths() {
        let script = build_forwarding_hook_script("C:\\Users\\me\\hooks", "pre-commit");

        assert!(script.contains("[A-Za-z]:/*|//*)"));
        assert!(script.contains("[A-Za-z]:\\\\*|\\\\\\\\*)"));
        assert!(
            script.contains("previous_hook_path=\"$previous_hooks_path\\\\$previous_hook_name\"")
        );
        assert!(script.contains("previous_hooks_path='C:\\Users\\me\\hooks'"));
    }

    #[test]
    fn forwarding_wrapper_recognizes_unc_absolute_paths() {
        let script = build_forwarding_hook_script("\\\\server\\share\\hooks", "pre-commit");

        assert!(script.contains("[A-Za-z]:\\\\*|\\\\\\\\*)"));
        assert!(
            script.contains("previous_hook_path=\"$previous_hooks_path\\\\$previous_hook_name\"")
        );
        assert!(script.contains("previous_hooks_path='\\\\server\\share\\hooks'"));
    }

    #[test]
    fn resolves_tilde_hooks_path_for_ownership_checks() {
        let repo_context = PathBuf::from("/tmp/example-repo");
        let home_dir = std::env::temp_dir().join(format!(
            "diddo-home-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));

        fs::create_dir_all(home_dir.join(".config/diddo/hooks")).unwrap();

        let resolved = resolve_hooks_path_for_comparison(
            "~/.config/diddo/hooks",
            &repo_context,
            Some(home_dir.as_path()),
        )
        .unwrap();

        assert_eq!(
            fs::canonicalize(resolved).unwrap(),
            fs::canonicalize(home_dir.join(".config/diddo/hooks")).unwrap()
        );
    }

    #[test]
    fn resolves_relative_hooks_path_against_repo_context_for_ownership_checks() {
        let repo_context = std::env::temp_dir().join(format!(
            "diddo-repo-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let hooks_dir = repo_context.join(".githooks");

        fs::create_dir_all(&hooks_dir).unwrap();

        let resolved = resolve_hooks_path_for_comparison(".githooks", &repo_context, None).unwrap();

        assert_eq!(
            fs::canonicalize(resolved).unwrap(),
            fs::canonicalize(hooks_dir).unwrap()
        );
    }

    #[test]
    fn install_with_creates_managed_hook_and_sets_global_hooks_path() {
        let temp = temp_dir("install-basic");
        let hooks_dir = temp.join("managed-hooks");
        let paths = test_paths(hooks_dir.clone());
        let configured_path = Arc::new(Mutex::new(None::<PathBuf>));
        let configured_path_for_assert = Arc::clone(&configured_path);

        install_with(
            &paths,
            || Ok(None),
            move |managed_dir| {
                *configured_path.lock().unwrap() = Some(managed_dir.to_path_buf());
                Ok(())
            },
        )
        .unwrap();

        let post_commit = hooks_dir.join("post-commit");
        let state_file = hooks_dir.join(STATE_FILE);

        assert!(post_commit.exists());
        assert_eq!(
            fs::read_to_string(post_commit).unwrap(),
            build_post_commit_script(None)
        );
        assert_eq!(fs::read_to_string(state_file).unwrap(), "");
        assert_eq!(
            configured_path_for_assert.lock().unwrap().clone(),
            Some(hooks_dir)
        );
    }

    #[test]
    fn install_with_preserves_previous_hooks_dir_behavior_and_records_previous_path() {
        let temp = temp_dir("install-chain");
        let managed_hooks_dir = temp.join("managed-hooks");
        let paths = test_paths(managed_hooks_dir.clone());
        let previous_hooks_path = temp.join("existing-hooks").display().to_string();

        install_with(
            &paths,
            || {
                Ok(Some(HookPathState {
                    raw: previous_hooks_path.clone(),
                    resolved: PathBuf::from(&previous_hooks_path),
                }))
            },
            |_managed_dir| Ok(()),
        )
        .unwrap();

        let generated_post_commit = managed_hooks_dir.join("post-commit");
        let generated_pre_commit = managed_hooks_dir.join("pre-commit");
        let state_file = managed_hooks_dir.join(STATE_FILE);

        assert_eq!(
            fs::read_to_string(&generated_post_commit).unwrap(),
            build_post_commit_script(Some(&previous_hooks_path))
        );
        assert!(generated_pre_commit.exists());
        assert!(
            fs::read_to_string(&generated_pre_commit)
                .unwrap()
                .contains("previous_hook_name='pre-commit'")
        );
        assert_eq!(
            fs::read_to_string(state_file).unwrap(),
            format!("{previous_hooks_path}\n")
        );
    }

    #[test]
    fn install_with_preserves_relative_previous_hooks_path_in_metadata_and_wrappers() {
        let temp = temp_dir("install-relative");
        let managed_hooks_dir = temp.join("managed-hooks");
        let paths = test_paths(managed_hooks_dir.clone());

        install_with(
            &paths,
            || {
                Ok(Some(HookPathState {
                    raw: ".githooks".to_string(),
                    resolved: PathBuf::from(".githooks"),
                }))
            },
            |_managed_dir| Ok(()),
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(managed_hooks_dir.join(STATE_FILE)).unwrap(),
            ".githooks\n"
        );
        assert!(
            fs::read_to_string(managed_hooks_dir.join("pre-commit"))
                .unwrap()
                .contains("previous_hooks_path='.githooks'")
        );
        assert!(
            fs::read_to_string(managed_hooks_dir.join("post-commit"))
                .unwrap()
                .contains("previous_hook_path=\"$PWD/$previous_hooks_path/$previous_hook_name\"")
        );
    }

    #[test]
    fn uninstall_with_restores_previous_hooks_path_when_diddo_still_owns_global_setting() {
        let temp = temp_dir("uninstall");
        let hooks_dir = temp.join("managed-hooks");
        let paths = test_paths(hooks_dir.clone());
        let restored_path = Arc::new(Mutex::new(None::<String>));
        let restored_path_for_assert = Arc::clone(&restored_path);
        let unset_called = Arc::new(Mutex::new(false));
        let unset_called_for_assert = Arc::clone(&unset_called);
        let current_hooks_dir = hooks_dir.clone();

        fs::create_dir_all(&hooks_dir).unwrap();
        fs::write(hooks_dir.join("post-commit"), "#!/bin/sh\ndiddo hook\n").unwrap();
        fs::write(hooks_dir.join(STATE_FILE), "/tmp/previous-hooks\n").unwrap();

        uninstall_with(
            &paths,
            move || {
                Ok(Some(HookPathState {
                    raw: current_hooks_dir.display().to_string(),
                    resolved: current_hooks_dir.clone(),
                }))
            },
            move |raw_path: &str| {
                *restored_path.lock().unwrap() = Some(raw_path.to_string());
                Ok(())
            },
            move || {
                *unset_called.lock().unwrap() = true;
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(
            restored_path_for_assert.lock().unwrap().clone(),
            Some("/tmp/previous-hooks".to_string())
        );
        assert!(!*unset_called_for_assert.lock().unwrap());
        assert!(!hooks_dir.exists());
    }

    #[test]
    fn uninstall_with_leaves_newer_global_hooks_path_untouched() {
        let temp = temp_dir("uninstall-owned-by-someone-else");
        let hooks_dir = temp.join("managed-hooks");
        let paths = test_paths(hooks_dir.clone());
        let restored_path = Arc::new(Mutex::new(None::<String>));
        let restored_path_for_assert = Arc::clone(&restored_path);
        let unset_called = Arc::new(Mutex::new(false));
        let unset_called_for_assert = Arc::clone(&unset_called);

        fs::create_dir_all(&hooks_dir).unwrap();
        fs::write(hooks_dir.join("post-commit"), "#!/bin/sh\ndiddo hook\n").unwrap();
        fs::write(hooks_dir.join(STATE_FILE), "/tmp/previous-hooks\n").unwrap();

        uninstall_with(
            &paths,
            || {
                Ok(Some(HookPathState {
                    raw: "/tmp/newer-hooks".to_string(),
                    resolved: PathBuf::from("/tmp/newer-hooks"),
                }))
            },
            move |raw_path: &str| {
                *restored_path.lock().unwrap() = Some(raw_path.to_string());
                Ok(())
            },
            move || {
                *unset_called.lock().unwrap() = true;
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(restored_path_for_assert.lock().unwrap().clone(), None);
        assert!(!*unset_called_for_assert.lock().unwrap());
        assert!(!hooks_dir.exists());
    }

    #[test]
    fn install_local_hook_adds_post_commit_to_repo_with_local_hooks_path() {
        let temp = temp_dir("local-hooks");
        let husky_dir = temp.join(".husky");
        fs::create_dir_all(&husky_dir).unwrap();

        Command::new("git")
            .args(["init"])
            .current_dir(&temp)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "core.hooksPath", ".husky"])
            .current_dir(&temp)
            .output()
            .unwrap();

        let paths = test_paths(temp.join("managed-hooks"));
        install_local_hook(&paths, &husky_dir, ".husky").unwrap();

        let post_commit = husky_dir.join("post-commit");
        assert!(post_commit.exists());
        let script = fs::read_to_string(&post_commit).unwrap();
        assert!(script.contains("diddo hook"));
        assert!(script.contains(super::DIDDO_MANAGED_MARKER));
    }

    #[test]
    fn install_local_hook_chains_existing_post_commit_in_local_hooks_dir() {
        let temp = temp_dir("local-hooks-chain");
        let husky_dir = temp.join(".husky");
        fs::create_dir_all(&husky_dir).unwrap();

        let existing_post_commit = husky_dir.join("post-commit");
        fs::write(&existing_post_commit, "#!/bin/sh\necho 'custom hook'\n").unwrap();

        Command::new("git")
            .args(["init"])
            .current_dir(&temp)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "core.hooksPath", ".husky"])
            .current_dir(&temp)
            .output()
            .unwrap();

        let paths = test_paths(temp.join("managed-hooks"));
        install_local_hook(&paths, &husky_dir, ".husky").unwrap();

        let post_commit = husky_dir.join("post-commit");
        let prev_path = husky_dir.join(POST_COMMIT_DIDDO_PREV);
        assert!(post_commit.exists());
        assert!(prev_path.exists());
        assert_eq!(
            fs::read_to_string(&prev_path).unwrap(),
            "#!/bin/sh\necho 'custom hook'\n"
        );
        let script = fs::read_to_string(&post_commit).unwrap();
        assert!(script.contains("diddo hook"));
        assert!(script.contains(POST_COMMIT_DIDDO_PREV));
    }

    #[test]
    fn install_local_hook_preserves_chain_when_re_run_on_diddo_managed_hook() {
        let temp = temp_dir("local-hooks-rerun");
        let husky_dir = temp.join(".husky");
        fs::create_dir_all(&husky_dir).unwrap();

        let existing_post_commit = husky_dir.join("post-commit");
        fs::write(&existing_post_commit, "#!/bin/sh\necho 'custom hook'\n").unwrap();

        Command::new("git")
            .args(["init"])
            .current_dir(&temp)
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "core.hooksPath", ".husky"])
            .current_dir(&temp)
            .output()
            .unwrap();

        let paths = test_paths(temp.join("managed-hooks"));
        install_local_hook(&paths, &husky_dir, ".husky").unwrap();
        let prev_path = husky_dir.join(POST_COMMIT_DIDDO_PREV);
        assert!(prev_path.exists());

        install_local_hook(&paths, &husky_dir, ".husky").unwrap();

        let post_commit = husky_dir.join("post-commit");
        let script = fs::read_to_string(&post_commit).unwrap();
        assert!(script.contains(POST_COMMIT_DIDDO_PREV));
        assert_eq!(
            fs::read_to_string(&prev_path).unwrap(),
            "#!/bin/sh\necho 'custom hook'\n"
        );
    }

    fn temp_dir(prefix: &str) -> PathBuf {
        let unique = format!(
            "{prefix}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let path = std::env::temp_dir().join(unique);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn test_paths(hooks_dir: PathBuf) -> AppPaths {
        AppPaths {
            db_path: hooks_dir.join("ignored.db"),
            config_path: hooks_dir.join("ignored.toml"),
            hooks_dir,
        }
    }
}

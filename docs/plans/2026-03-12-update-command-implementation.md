# diddo update Command — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add `diddo update [--yes]` that self-updates by detecting Homebrew vs GitHub install and updating via `brew upgrade diddo` or GitHub Releases.

**Architecture:** New `update` module with install-type detection (Homebrew prefix / path contains `Cellar`), target-triple mapping for release assets, version comparison (semver), and two code paths: Homebrew runs `brew upgrade diddo`; GitHub uses `self_update` crate (or its building blocks) to download the correct asset and replace the running binary. Prompt "Update diddo X → Y? [y/N]" unless `--yes` or non-TTY.

**Tech Stack:** Rust, clap, `self_update` crate, `semver` crate (or minimal semver comparison), `std::env::current_exe`, `std::process::Command` for Homebrew.

**Design reference:** `docs/plans/2026-03-12-update-command-design.md`

---

## Task 1: Add dependency and Update CLI surface

**Files:**
- Modify: `Cargo.toml`
- Modify: `src/main.rs` (add `mod update`, `Update` variant, `UpdateArgs`, dispatch, and `summary_request_from_cli` exclusion)

**Step 1: Add crates**

In `Cargo.toml`, add under `[dependencies]`:

```toml
self_update = "0.36"
semver = "1.0"
```

(Use latest compatible versions from crates.io; `self_update` 0.36+ supports GitHub and replace logic.)

**Step 2: Add module and Update subcommand**

In `src/main.rs`:
- Add `mod update;` with other `mod` lines.
- In `Commands` enum, add variant (after `Metadata`):

```rust
/// Update diddo to the latest release.
Update(UpdateArgs),
```

- Define args struct (can be in main.rs or in update.rs; for simplicity keep in main.rs next to other command args). Add after `SummaryArgs`:

```rust
#[derive(Args, Debug, Clone, Copy, PartialEq, Eq)]
struct UpdateArgs {
    /// Apply update without prompting.
    #[arg(long, alias = "assume-yes")]
    yes: bool,
}
```

- In `run_cli`, add branch before `_ => run_summary_command`:

```rust
Some(Commands::Update(args)) => run_update_command(args),
```

- In `summary_request_from_cli`, add `Commands::Update(_)` to the `None` arm (so Update is not treated as a summary request):

```rust
Some(Commands::Init | Commands::Uninstall | Commands::Hook | Commands::Config | Commands::Metadata | Commands::Update(_)) => None,
```

- Add stub:

```rust
fn run_update_command(args: UpdateArgs) -> Result<(), Box<dyn Error>> {
    update::run(args.yes)
}
```

**Step 3: Create update module stub**

Create `src/update.rs`:

```rust
use std::error::Error;

pub fn run(assume_yes: bool) -> Result<(), Box<dyn Error>> {
    // TODO: implement
    let _ = assume_yes;
    Ok(())
}
```

**Step 4: Build**

Run: `cargo build`
Expected: SUCCESS (with possible dead_code on `assume_yes`).

**Step 5: Commit**

```bash
git add Cargo.toml src/main.rs src/update.rs
git commit -m "chore: add update subcommand surface and self_update/semver deps"
```

---

## Task 2: Install type detection and target triple

**Files:**
- Create: `src/update.rs` (replace stub with detection and target logic)
- Test: unit tests in `src/update.rs` (or `src/update/mod.rs` later)

**Step 1: Define install type and target**

In `src/update.rs`, add:

- Enum `InstallType { Homebrew, GitHub }`.
- Function `fn current_install_type(exe_path: &Path) -> InstallType`:
  - Canonicalize `exe_path` if possible.
  - Run `brew --prefix` (capture output); if success, check if canonical exe path starts with prefix path (normalize both). If yes, return `InstallType::Homebrew`.
  - If path string contains `"Cellar"`, return `InstallType::Homebrew`.
  - Else return `InstallType::GitHub`.
- Function `fn release_target() -> Option<&'static str>`: use `std::env::consts::OS` and `std::env::consts::ARCH` to return the target string from the design doc table (e.g. `aarch64-apple-darwin`, `x86_64-unknown-linux-gnu`, `x86_64-pc-windows-msvc`). Return `None` for unsupported (e.g. other OS/arch).

**Step 2: Write failing tests**

Add `#[cfg(test)] mod tests` in `src/update.rs`:

- `test_install_type_homebrew_when_path_under_prefix`: mock by passing a path under a temp dir that you treat as "prefix"; or run only when `brew --prefix` works and current exe is under it (env test). Prefer: pass path like `/opt/homebrew/Cellar/diddo/0.5.0/bin/diddo`, no `brew` call in test; have a helper that only checks path (e.g. `install_type_from_path(path)` that checks path contains `Cellar` or is under a given prefix). So split: `install_type_from_path(path: &Path, brew_prefix: Option<&Path>) -> InstallType`.
- `test_install_type_github_when_path_not_homebrew`: path `/usr/local/bin/diddo` and prefix `None` or `/opt/homebrew` → GitHub.
- `test_release_target_matches_expected_for_platform`: for current (OS, ARCH), `release_target()` returns the expected string from the design table (one test per platform or one parameterized; on CI only current platform runs).

**Step 3: Run tests**

Run: `cargo test update::`
Expected: FAIL (functions not implemented or return wrong type).

**Step 4: Implement**

- Implement `release_target()` with a match on `(OS, ARCH)`.
- Implement `install_type_from_path(path, brew_prefix)` and in `run()` call `current_exe()`, then get brew prefix via `Command::new("brew").arg("--prefix").output()`; pass to `install_type_from_path`.
- Wire `run(assume_yes)` to only call detection and return Ok(()) for now.

**Step 5: Run tests**

Run: `cargo test update::`
Expected: PASS.

**Step 6: Commit**

```bash
git add src/update.rs
git commit -m "feat(update): detect Homebrew vs GitHub and map to release target"
```

---

## Task 3: Version comparison and “already up to date”

**Files:**
- Modify: `src/update.rs`

**Step 1: Add version comparison**

- Add `fn is_newer(current: &str, latest: &str) -> bool`. Strip leading `v` from both; use `semver::Version::parse` for each; compare. If either parse fails, treat as not newer (or use simple lexicographic compare for tags like `0.5.0`). Prefer semver so `0.6.0` > `0.5.0`.
- For Homebrew: get “latest” version. Option A: run `brew info diddo` and parse version from output. Option B: fetch GitHub API `repos/drugoi/diddo-hooks/releases/latest` and use tag. Option B is consistent; use GitHub API for “latest version” in both paths so we have one source of truth.
- Add helper `fn fetch_latest_release_tag() -> Result<String, Box<dyn Error>>`: GET `https://api.github.com/repos/drugoi/diddo-hooks/releases/latest`, parse JSON for `tag_name`, return without `v` (e.g. `0.6.0`). Use `reqwest::blocking::get` and `json`. User-Agent header required by GitHub (e.g. `diddo/0.5.0`).

**Step 2: Write failing tests**

- `test_is_newer`: `is_newer("0.5.0", "0.6.0")` true; `is_newer("0.5.0", "0.5.0")` false; `is_newer("0.6.0", "0.5.0")` false; `is_newer("0.5.0", "v0.6.0")` true (strip v).
- Optional: mock HTTP for `fetch_latest_release_tag` (e.g. with `mockito`) to return a JSON body; assert tag parsed. If skipped, rely on manual test.

**Step 3: Run tests**

Run: `cargo test update::`
Expected: FAIL for new tests, then implement.

**Step 4: Implement**

- Implement `is_newer` with semver; strip `v` before parse.
- Implement `fetch_latest_release_tag` with reqwest; set User-Agent. Parse `tag_name`, trim `v`.
- In `run(assume_yes)`: after detecting install type, call `fetch_latest_release_tag()`. Current version = `env!("CARGO_PKG_VERSION")`. If `!is_newer(current, &latest)`, print "diddo is already up to date ({}).", current, return Ok(()).

**Step 5: Run tests**

Run: `cargo test update::` and `cargo build`
Expected: PASS.

**Step 6: Commit**

```bash
git add src/update.rs
git commit -m "feat(update): version comparison and already-up-to-date check"
```

---

## Task 4: Prompt helper and non-TTY behavior

**Files:**
- Modify: `src/update.rs`

**Step 1: Add prompt helper**

- `fn confirm_update(current: &str, latest: &str, assume_yes: bool) -> bool`: if `assume_yes`, return true. If `!std::io::stdin().is_terminal()`, print "A new version is available. Run with --yes to update non-interactively." and return false. Else print "Update diddo {} → {}? [y/N] ", current, latest; read one character (e.g. line or single key); return true only for y/Y.
- Use in both Homebrew and GitHub paths before applying update.

**Step 2: Test**

- Unit test: when `assume_yes` is true, `confirm_update` returns true without reading. When assume_yes false and stdin not TTY, returns false and message can be asserted (or test with a mock stdin). Optional: test with piped "y" to assert true.

**Step 3: Run tests**

Run: `cargo test update::`
Expected: PASS after implementation.

**Step 4: Commit**

```bash
git add src/update.rs
git commit -m "feat(update): add update confirmation prompt and --yes / non-TTY handling"
```

---

## Task 5: Homebrew update path

**Files:**
- Modify: `src/update.rs`

**Step 1: Implement Homebrew flow**

In `run(assume_yes)`:
- After install type and version check, if `InstallType::Homebrew`:
  - If not `confirm_update(current, &latest, assume_yes)` return Ok(()).
  - Run `Command::new("brew").args(["upgrade", "diddo"]).status()`. If not success, print "Update failed: brew upgrade diddo failed." and return Err.
  - Print "Updated to {}.", latest; return Ok(()).
- If `brew` not in PATH when we chose Homebrew (e.g. we detected by path containing Cellar but never ran `brew --prefix`), before running upgrade check `which brew` or run `brew --version`; if failure, return error "Homebrew update requested but `brew` not found."

**Step 2: Manual test**

Run from a Homebrew-installed diddo (or symlink under Cellar): `diddo update` and `diddo update --yes`. Expect upgrade or “already up to date”.

**Step 3: Commit**

```bash
git add src/update.rs
git commit -m "feat(update): run brew upgrade diddo for Homebrew installs"
```

---

## Task 6: GitHub update path with self_update

**Files:**
- Modify: `src/update.rs`, possibly `Cargo.toml` (features for self_update)

**Step 1: Configure self_update**

In `run(assume_yes)`, for `InstallType::GitHub`:
- After version check, if !`confirm_update(current, &latest, assume_yes)` return Ok(()).
- Get `release_target()`; if None, return error "No release available for your platform."
- Build asset name: Unix `diddo-{latest}-{target}.tar.gz`, Windows `diddo-{latest}-{target}.zip`.
- Use `self_update::backends::github::Update::configure()`
  - `.repo_owner("drugoi").repo_name("diddo-hooks").bin_name("diddo")`
  - `.current_version(current).target(target)`
  - If the crate expects a different asset name pattern, use the crate’s API to download by asset name (e.g. list assets, find by name, download URL from asset, then use self_update’s replace or `self_replace`). Docs: https://docs.rs/self_update/
- Call `.build()?.update()?`. On success print "Updated to {}.", latest. Map errors to the messages from the design doc (download failed, replace failed).

**Step 2: Handle replace errors**

Ensure on replace failure we do not delete the current binary; self_update should write to temp then rename. If the crate does not support our exact asset names, implement: fetch release JSON, find asset with name == expected filename, download asset URL to temp file, extract (tar.gz or zip), find binary inside, replace current exe using crate’s replace API or a small script (Unix: write temp, chmod +x, spawn script that sleeps 1, mv temp current_exe, exec current_exe).

**Step 3: Build and test**

Run: `cargo build --release`
Run: `cargo test update::`
Optional: run `diddo update` from a non-Homebrew binary (e.g. cargo run) and confirm it attempts download (may fail in sandbox).

**Step 4: Commit**

```bash
git add src/update.rs Cargo.toml
git commit -m "feat(update): download and replace from GitHub Releases for non-Homebrew"
```

---

## Task 7: Error messages and edge cases

**Files:**
- Modify: `src/update.rs`

**Step 1: Align errors with design**

- Network/API: "Could not check for updates: {}."
- No asset for target: "No release available for your platform ({})."
- Download failure: "Download failed: {}."
- Replace failure: "Update failed: could not replace binary ({}). You can download the new version from https://github.com/drugoi/diddo-hooks/releases."
- Brew not found: "Homebrew update requested but `brew` not found."
- Permissions: clear message, exit 1.

**Step 2: Run full test suite**

Run: `cargo test`
Expected: All pass.

**Step 3: Commit**

```bash
git add src/update.rs
git commit -m "feat(update): standardize error messages per design"
```

---

## Task 8: Integration and docs

**Files:**
- Modify: `src/main.rs` (ensure Update appears in help)
- Modify: `docs/plans/2026-03-12-update-command-design.md` if needed (no code changes required)
- Verify: `cargo run -- update --help` and `cargo run -- --help`

**Step 1: Help text**

Run: `cargo run -- update --help`
Expected: "Update diddo to the latest release" and `--yes` / `--assume-yes` shown.

**Step 2: README or AGENTS.md**

If project has a README with command list, add `diddo update` and one line description. AGENTS.md: add bullet under behavior that diddo can self-update via `diddo update`.

**Step 3: Final commit**

```bash
git add src/main.rs AGENTS.md
git commit -m "docs: document diddo update in help and AGENTS.md"
```

---

## Execution handoff

Plan complete and saved to `docs/plans/2026-03-12-update-command-implementation.md`.

Two execution options:

1. **Subagent-driven (this session)** — I dispatch a fresh subagent per task, review between tasks, fast iteration.
2. **Parallel session (separate)** — Open a new session with executing-plans and run in a worktree with checkpoints.

Which approach do you want?

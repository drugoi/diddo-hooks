# diddo update — Design Document

Self-update command that detects how diddo was installed and updates via Homebrew or GitHub Releases.

## Scope

- **Command:** `diddo update [--yes]`
  - `--yes` / `--assume-yes`: apply update without prompting.
- **Supported install types:** Homebrew, or downloaded binary (GitHub Releases). If detection is ambiguous, assume GitHub.
- **Unsupported:** No special handling for `cargo install`; such installs are treated as GitHub and may be overwritten by the release binary.

## Installation detection

1. Resolve current executable path (e.g. `std::env::current_exe()`; canonicalize symlinks).
2. **Homebrew:** current path is under the Homebrew prefix, or path contains the segment `Cellar`.
   - Get prefix via `brew --prefix`; if `brew` is missing or path is not under prefix and does not contain `Cellar`, treat as GitHub.
3. **GitHub:** any other case (including when `brew` is not in PATH).

## Behavior by install type

### Homebrew

- Optionally determine latest available version (e.g. `brew info diddo` or GitHub API) and compare to `env!("CARGO_PKG_VERSION")`. If not newer, print "diddo is already up to date" and exit 0.
- If newer: unless `--yes`, prompt "A new version of diddo is available (X → Y). Update? [y/N]"; if not y/Y, exit 0.
- Run `brew upgrade diddo`. On success print "Updated to Y."; on failure print error and exit non-zero.
- If `brew` is not found after we chose Homebrew path: "Homebrew update requested but `brew` not found." Exit 1.

### GitHub

- Fetch latest release: `GET /repos/drugoi/diddo-hooks/releases/latest`. On network/API error, clear message and exit 1.
- Parse tag (strip `v`), compare to current version (semver). If not newer, print "diddo is already up to date (X)." and exit 0.
- Pick asset for current target (see Asset selection). If missing: "No release available for your platform (<target>)." Exit 1.
- Unless `--yes`, prompt "Update diddo X → Y? [y/N]"; if not y/Y, exit 0.
- Download asset to temp file, extract, replace current binary (prefer `self_update` crate for replace logic). On failure: leave existing binary unchanged, print error and exit 1.

### Non-TTY

When stdin is not a TTY and an update is available, do not prompt. Print "A new version is available. Run with --yes to update non-interactively." and exit 0.

## Asset selection

Map current platform to release target and asset filename (must match `.github/workflows/release.yml`):

| OS     | ARCH    | Target                      | Asset |
|--------|---------|-----------------------------|-------|
| macOS  | aarch64 | aarch64-apple-darwin        | diddo-{version}-aarch64-apple-darwin.tar.gz |
| macOS  | x86_64  | x86_64-apple-darwin         | diddo-{version}-x86_64-apple-darwin.tar.gz |
| Linux  | aarch64 | aarch64-unknown-linux-gnu   | diddo-{version}-aarch64-unknown-linux-gnu.tar.gz |
| Linux  | x86_64  | x86_64-unknown-linux-gnu     | diddo-{version}-x86_64-unknown-linux-gnu.tar.gz |
| Windows| x86_64  | x86_64-pc-windows-msvc       | diddo-{version}-x86_64-pc-windows-msvc.zip |

Version from latest release tag (e.g. `v0.6.0` → `0.6.0`). Select the asset whose name equals the expected filename for the current target.

## Prompts and flags

- Single prompt: "Update diddo X → Y? [y/N]". Default N. Only when a newer version exists (and for GitHub, asset is available).
- `--yes` / `--assume-yes`: skip prompt and apply update.
- No separate "check only" in this design.

## Error handling

| Situation           | Message / behavior |
|---------------------|--------------------|
| Network / API       | "Could not check for updates: <reason>." Exit 1. |
| No asset for target | "No release available for your platform (<target>)." Exit 1. |
| Download failure    | "Download failed: <reason>." Exit 1. |
| Replace failure     | "Update failed: could not replace binary (<reason>). You can download the new version from <releases URL>." Exit 1. |
| `brew` not found    | "Homebrew update requested but `brew` not found." Exit 1. |
| Homebrew upgrade fails | Print brew stderr, "Update failed: brew upgrade diddo failed." Exit 1. |
| Permissions         | Clear message, exit 1; do not overwrite binary partially. |

## Implementation

### Approach

Use the `self_update` crate for the GitHub path (download, extract, replace-self). Implement Homebrew detection first; when detected, run `brew upgrade diddo` with prompt unless `--yes`. When not Homebrew, use `self_update` configured for repo `drugoi/diddo-hooks`, binary name `diddo`, and asset naming above (or lower-level APIs if default naming does not match).

### Structure

- **CLI:** Add `Update(UpdateArgs)` to `Commands` in `main.rs`; `UpdateArgs { assume_yes: bool }`. Dispatch to `run_update_command(args)`.
- **Module:** `src/update.rs` (or `src/update/`): install type detection, target mapping, version comparison, prompt helper, Homebrew execution, GitHub flow (with `self_update` or reqwest + replace).
- **Dependencies:** Add `self_update` to `Cargo.toml`. Use semver comparison (e.g. `semver` crate or simple major.minor.patch) for version check.

### Dependency injection for tests

Pass into update logic: current version (default `env!("CARGO_PKG_VERSION")`), current exe path (default `std::env::current_exe()`), stdin TTY check, and optionally path to `brew` (or closure). Enables unit tests without network or real replace.

## Testing

- **Unit:** Install detection (path under Homebrew prefix or containing `Cellar` → Homebrew; else → GitHub). Target mapping for each supported (OS, arch). Version comparison (newer / same / older; tag with `v`). Asset filename construction.
- **Edge cases:** Non-TTY → no prompt, suggest `--yes`. `--yes` → no prompt. Already up to date → exit 0, no prompt.
- **Optional integration:** Mock GitHub API response; assert correct asset URL and no replace when not newer. No real download/replace in CI.
- **Manual:** Run `diddo update` and `diddo update --yes` on dev/release build for Homebrew and GitHub installs.

## Success criteria

- Homebrew users: `diddo update` runs `brew upgrade diddo` with prompt (or `--yes`).
- Non-Homebrew users: in-place update from GitHub Releases with prompt (or `--yes`).
- Already latest: exit 0 with "already up to date".
- All failure paths: clear message and non-zero exit; existing binary unchanged on replace failure.

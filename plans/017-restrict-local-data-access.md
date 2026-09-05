# Plan 017: Owner-only permissions for config/database; atomic, symlink-proof activity export

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 7a8b4ca..HEAD -- src/db.rs src/activity_report.rs src/main.rs`
> Plan 004 has likely modified `Database::open`/`initialize` — expected;
> apply Step 1 to the live shape. If `export_markdown_to_dir` /
> `unique_export_path` changed, compare excerpts; on mismatch, STOP.

## Status

- **Priority**: P2
- **Effort**: S-M
- **Risk**: LOW
- **Depends on**: plans/004-harden-sqlite-open-path.md (edits the same `Database::open`; land 004 first)
- **Category**: security
- **Planned at**: commit `7a8b4ca`, 2026-09-05

## Why this matters

1. **World-readable secrets and history**: `commits.db` (every commit message, repo path, branch, and author email across all the user's repos) is created via `fs::create_dir_all` + `Connection::open` with default modes (0644 & ~umask); `config.toml` can hold `ai.api_key` in plaintext (a documented option — but the *file being group/world-readable* is not part of that convention). On shared hosts, CI runners, or multi-account machines, other local users can read both. The only permission handling in the codebase today is `set_executable_if_unix` for hook scripts.
2. **Exists-then-write export**: the activity report's `e` keybinding writes `diddo_activity_{months}_month_{date}.md` into the **current directory** using `!path.exists()` followed by `fs::write`. `exists()` follows symlinks and `fs::write` truncates through them: a dangling symlink planted at the predictable name redirects the write to an attacker-chosen path; it is also a plain TOCTOU race.

## Current state

- `src/db.rs` `Database::open` (post-plan-004 shape; originally lines 54–67): `fs::create_dir_all(parent)` then `Connection::open(path)`. No permission calls.
- `src/activity_report.rs:337-357`:

```rust
fn unique_export_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    ...
    for n in 2..=9999 {
        let candidate = parent.join(format!("{stem}_{n}.{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    parent.join(format!("{stem}_{}.{ext}", std::process::id()))
}
```

- `src/activity_report.rs:428-444`:

```rust
pub fn export_markdown(report: &ActivityReport) -> Result<PathBuf, std::io::Error> {
    export_markdown_to_dir(report, Path::new("."))
}

fn export_markdown_to_dir(report: &ActivityReport, directory: &Path) -> Result<PathBuf, std::io::Error> {
    let today = Local::now().date_naive();
    let path = unique_export_path(&directory.join(format!(
        "diddo_activity_{}_month_{}.md", report.period_months, today
    )));
    let content = render_markdown(report);
    fs::write(&path, content)?;
    Ok(path)
}
```

- Caller: `src/interactive.rs:330-334` (the `e` key). Existing export tests use `export_markdown_to_dir` with a temp dir (find with `grep -n 'export_markdown_to_dir' src/activity_report.rs`).
- Permission-handling exemplar: `set_executable_if_unix` in `src/init.rs` (~lines 669–685) — `#[cfg(unix)]` + `PermissionsExt`; follow its style.
- `diddo metadata` output is built by `format_metadata` (`src/main.rs`, returns the multi-line string ending with `Auto-update:` — see lines 470–486); config path available there.
- Config loading: `AppConfig::load(&paths.config_path)` (`src/config.rs:74-89`) — read-only; diddo never *creates* config.toml, the user does. So config protection is: warn when insecure (metadata command), and set the *directory* mode when diddo creates directories.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Targeted tests | `cargo test db:: && cargo test activity_report::` | all pass |
| Full suite | `cargo test` | 0 failed |
| Lint/format | `cargo clippy -- -D warnings && cargo fmt -- --check` | exit 0 |

## Scope

**In scope**:
- `src/db.rs` — dir/file modes at creation (unix-gated)
- `src/activity_report.rs` — atomic `create_new` export
- `src/main.rs` — one warning line in `format_metadata` when config is insecure and holds a key
- `src/paths.rs` — ONLY if a shared `#[cfg(unix)]` helper naturally lives there; otherwise don't touch

**Out of scope**:
- Windows ACLs (no-op on non-unix, like `set_executable_if_unix`).
- Changing the export destination directory or adding an `--output` flag (direction finding, not planned).
- Encrypting anything; keychain integration.
- `config.toml` creation/chmod — diddo doesn't create it; we warn instead.

## Git workflow

- Branch: `advisor/017-restrict-local-data-access`
- Commit style: conventional commits, e.g. `fix: create data files owner-only and export atomically`. No AI attribution.
- Do NOT push or open a PR unless instructed.

## Steps

### Step 1: Owner-only modes at creation (db.rs)

In `Database::open`, after `fs::create_dir_all(parent)` succeeds, set the directory to 0700; after `Connection::open(path)` returns (file now exists), set the DB file to 0600 — both unix-gated, both best-effort-with-propagation choice: propagate errors (they only fire on exotic filesystems; a silent failure would defeat the purpose). Follow the `set_executable_if_unix` idiom:

```rust
#[cfg(unix)]
fn restrict_to_owner(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(mode);
    std::fs::set_permissions(path, perms)
}
```

Apply 0o700 to the parent dir, 0o600 to the db file. Map errors through the same error construction plan 004 introduced for dir-creation failures. WAL sidecars (`-wal`/`-shm`, created by SQLite after plan 004) inherit the 0700 directory protection; additionally chmod them 0600 if they exist (best-effort `let _ =` — they may not exist yet).

Note: tightening an *existing* user's 0755 data dir on next run is intended behavior, not just creation-time — apply unconditionally on open (idempotent, cheap: two metadata calls... it runs on every commit via the hook; acceptable — two stat+chmod syscalls are microseconds. Add a comment).

**Verify**: `cargo test db::` all pass. Manual: `rm -rf /tmp/dtest && DIDDO=1 cargo test db:: >/dev/null; ` — better: add the Step 3 test below; also on this machine run `ls -la "$(dirname "$(cargo run --quiet -- config | grep 'Database path' | cut -d: -f2- | xargs)")"` → directory mode `drwx------` after any command that opens the DB (e.g. `cargo run --quiet -- metadata`).

### Step 2: Atomic export (activity_report.rs)

Replace the check-then-write pair with create-new-or-retry:

```rust
fn export_markdown_to_dir(report: &ActivityReport, directory: &Path) -> Result<PathBuf, std::io::Error> {
    use std::io::Write;
    let today = Local::now().date_naive();
    let base = directory.join(format!(
        "diddo_activity_{}_month_{}.md", report.period_months, today
    ));
    let content = render_markdown(report);
    let mut path = base.clone();
    for n in 1..=9999u32 {
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                file.write_all(content.as_bytes())?;
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                path = numbered_variant(&base, n + 1); // reuse the stem/ext logic from unique_export_path
            }
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("could not find a free export filename"))
}
```

Extract the stem/ext suffixing из `unique_export_path` into `numbered_variant(base, n)`; delete `unique_export_path` and its `exists()` checks entirely (`create_new` is the atomic replacement: it refuses to follow an existing symlink — dangling or not — because the path must not exist at all).

**Verify**: `cargo test activity_report::` — update any test that called `unique_export_path`; all pass.

### Step 3: Tests

1. In `db.rs` (unix-gated `#[cfg(unix)] #[test]`): open a DB at a temp path (`std::env::temp_dir().join(unique_name)`), then assert dir mode `& 0o777 == 0o700` and file mode `& 0o777 == 0o600` via `PermissionsExt::mode()`. Clean up.
2. In `activity_report.rs`: `export_appends_suffix_when_file_exists` — pre-create the dated filename in a temp dir, export, assert the returned path has the `_2` suffix and both files exist (adapt the existing export tests' fixture-building).
3. `#[cfg(unix)]` `export_refuses_dangling_symlink` — `std::os::unix::fs::symlink("/nonexistent/target", &dated_path)`, export, assert the *symlink was not written through*: the returned path is the `_2` variant (create_new on the symlink path fails AlreadyExists → suffix) AND `/nonexistent/target`... just assert `fs::read_link(&dated_path)` still errors-or-points-to-nonexistent and the returned path ≠ dated_path.

### Step 4: Metadata warning for insecure config

In `format_metadata` (`src/main.rs`), when (unix) the config file exists, its mode has any group/other bits (`mode & 0o077 != 0`), AND a resolved API key came from the config file, append a line to the output:

```
Warning: config file is readable by other users and contains ai.api_key — run: chmod 600 <path>
```

Determining "key came from the config file" precisely: `config.ai.api_key.is_some()` after load but BEFORE env application — check how `AppConfig::load`/`apply_environment_defaults` interact (`src/config.rs:115-132`); if load already merges env, use the raw parsed struct's field. Simplest honest check: parse the file's text for `api_key` presence is fragile — prefer the struct field pre-env-merge; read `load`'s body to find the right seam. Add a test if `format_metadata` is testable with an injected config (check existing `metadata_shows_config_settings` test in main.rs for the pattern); otherwise verify manually with a temp config and note it.

**Verify**: `cargo test` → 0 failed.

## Test plan

Steps 3–4: ~4 new tests (2 unix-gated). Full gate `cargo test` 0 failed.

## Done criteria

- [ ] `cargo test` 0 failed incl. new tests
- [ ] `cargo clippy -- -D warnings`; `cargo fmt -- --check` exit 0
- [ ] `grep -c 'create_new(true)' src/activity_report.rs` → 1; `grep -c 'fn unique_export_path' src/activity_report.rs` → 0
- [ ] `grep -c '0o600' src/db.rs` ≥ 1 and `grep -c '0o700' src/db.rs` ≥ 1
- [ ] Manual: after `cargo run --quiet -- metadata`, the data directory lists as `drwx------`
- [ ] `git diff --name-only` ⊆ {src/db.rs, src/activity_report.rs, src/main.rs, plans/README.md}

## STOP conditions

Stop and report back if:

- Plan 004 has not landed (`Database::initialize` still runs unconditional schema) — dependency order.
- chmod propagation breaks tests on this machine's filesystem (e.g. tmpfs oddities) — report before downgrading to best-effort.
- The pre-env-merge seam for "key from config file" doesn't exist without refactoring `AppConfig::load` — implement the warning keyed on `resolved_api_key().is_some()` instead, note the imprecision (warns even when key came from env), and flag for review.

## Maintenance notes

- Future `diddo export`/`prune` features (direction findings) must preserve the 0600/0700 regime on anything they create.
- The export still targets the CWD by design (documented behavior gap is a docs finding); an `--output` flag is the eventual fix — deliberately not smuggled in here.
- Reviewer should scrutinize: that chmod on every open doesn't fight a user who deliberately loosened permissions — the code comment should state this is a security invariant, overriding manual loosening.

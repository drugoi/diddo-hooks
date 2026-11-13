# Plan 017: Atomic schema migration; owner-only permissions for config/database; atomic, symlink-proof activity export

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
- **Category**: security + bug
- **Planned at**: commit `7a8b4ca`, 2026-09-05
- **Revised**: 2026-09-06 at commit `2ffe5ac` (after plan 004 merged) — added Step 1, making
  the schema migration atomic. Folded in here by maintainer decision rather than given its own
  plan number, because it edits the same `Database::initialize` this plan already touches. The
  underlying check-then-`ALTER` race is pre-existing (it predates plan 004); plan 004 narrowed
  how often the probe runs but did not fix the race. Steps renumbered 1→2, 2→3, 3→4, 4→5.

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
- `src/db.rs` — atomic migration transaction (Step 1); dir/file modes at creation (unix-gated)
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

### Step 1: Make the schema migration atomic (db.rs)

Plan 004 left a check-then-act race in the migration path. `run_author_email_migration`
reads `PRAGMA table_info(commits)`, sees no `author_email`, then runs `ALTER TABLE`. On the
**first open after upgrading** a pre-`author_email` database, two `diddo hook` processes
starting at the same instant can both pass the check; the first `ALTER` wins and the second
fails with `duplicate column name: author_email`. That is a schema error, not a lock error,
so `busy_timeout` does not retry it — the losing hook exits non-zero and that commit is
never recorded. Same shape applies to the `user_version` read-then-stamp.

Fix: take the write lock *before* reading the version, so the whole read-migrate-stamp
sequence is serialized. In `initialize` (post-plan-004 shape), wrap only that sequence —
the pragmas must stay outside, because `PRAGMA journal_mode = WAL` cannot run inside a
transaction:

```rust
    fn initialize(mut connection: Connection) -> Result<Self> {
        // ... busy_timeout / journal_mode / synchronous pragmas unchanged, OUTSIDE the tx ...

        const SCHEMA_VERSION: i64 = 1;
        {
            // BEGIN IMMEDIATE takes the write lock up front, so a second process
            // blocks here (honoring busy_timeout) and then observes the already
            // stamped version instead of re-running the ALTER TABLE.
            let tx = connection
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let version: i64 = tx.query_row("PRAGMA user_version", [], |row| row.get(0))?;
            if version < SCHEMA_VERSION {
                tx.execute_batch(SCHEMA)?;
                run_author_email_migration(&tx)?;
                tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            } else if version > SCHEMA_VERSION {
                return Err(/* unchanged newer-version error from plan 004 */);
            }
            tx.commit()?;
        }

        Ok(Self { connection })
    }
```

Notes for the executor:
- `initialize` currently takes `connection: Connection` by value; change it to `mut connection` so `transaction_with_behavior` (which needs `&mut self`) can borrow it. The parameter is owned, so no caller changes.
- `run_author_email_migration(&tx)` compiles unchanged — `Transaction` derefs to `Connection`.
- `ALTER TABLE` and `PRAGMA user_version` are both transactional in SQLite; `PRAGMA journal_mode` is not. Keep the pragma block where plan 004 put it.
- Keep the newer-version error branch byte-for-byte as plan 004 wrote it, including its message. Returning early from inside the block rolls the transaction back, which is correct.

**Verify**: `cargo test db::` → all pass, including plan 004's `open_upgrades_version_zero_database`, `open_refuses_newer_schema_version`, `schema_version_is_stamped_after_open`, and `wal_mode_enabled_for_file_database`. If `wal_mode_enabled_for_file_database` fails, you have most likely moved a pragma inside the transaction — move it back out.

### Step 2: Owner-only modes at creation (db.rs)

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

### Step 3: Atomic export (activity_report.rs)

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

### Step 4: Tests

0. In `db.rs`, for Step 1: `reopening_a_migrated_database_is_a_noop` — create a legacy-shaped DB at a temp file path (raw `Connection`, the `commits`/`ai_summary_cache` tables WITHOUT `author_email`, `user_version` left at 0), insert one row, then call `Database::open` on that path **twice in a row**. Assert both calls return `Ok`, the row is still present, `author_email` now exists, and `user_version` is 1. This is the regression gate for the double-migration path; without the transaction the second open is still fine sequentially, so ALSO note in your report that the concurrent case is not covered by a deterministic test (see below).

   Optional, only if it is not flaky on this machine: spawn two `std::thread`s that each call `Database::open` on the same fresh legacy DB path and join both; assert both return `Ok`. Run it 20 times locally. If it ever fails or hangs, delete it and say so — a flaky test is worse than no test here.

1. In `db.rs` (unix-gated `#[cfg(unix)] #[test]`): open a DB at a temp path (`std::env::temp_dir().join(unique_name)`), then assert dir mode `& 0o777 == 0o700` and file mode `& 0o777 == 0o600` via `PermissionsExt::mode()`. Clean up.
2. In `activity_report.rs`: `export_appends_suffix_when_file_exists` — pre-create the dated filename in a temp dir, export, assert the returned path has the `_2` suffix and both files exist (adapt the existing export tests' fixture-building).
3. `#[cfg(unix)]` `export_refuses_dangling_symlink` — `std::os::unix::fs::symlink("/nonexistent/target", &dated_path)`, export, assert the *symlink was not written through*: the returned path is the `_2` variant (create_new on the symlink path fails AlreadyExists → suffix) AND `/nonexistent/target`... just assert `fs::read_link(&dated_path)` still errors-or-points-to-nonexistent and the returned path ≠ dated_path.

### Step 5: Metadata warning for insecure config

In `format_metadata` (`src/main.rs`), when (unix) the config file exists, its mode has any group/other bits (`mode & 0o077 != 0`), AND a resolved API key came from the config file, append a line to the output:

```
Warning: config file is readable by other users and contains ai.api_key — run: chmod 600 <path>
```

Determining "key came from the config file" precisely: `config.ai.api_key.is_some()` after load but BEFORE env application — check how `AppConfig::load`/`apply_environment_defaults` interact (`src/config.rs:115-132`); if load already merges env, use the raw parsed struct's field. Simplest honest check: parse the file's text for `api_key` presence is fragile — prefer the struct field pre-env-merge; read `load`'s body to find the right seam. Add a test if `format_metadata` is testable with an injected config (check existing `metadata_shows_config_settings` test in main.rs for the pattern); otherwise verify manually with a temp config and note it.

**Verify**: `cargo test` → 0 failed.

## Test plan

Steps 4–5: ~5 new tests (2 unix-gated). Full gate `cargo test` 0 failed.

## Done criteria

- [ ] `cargo test` 0 failed incl. new tests
- [ ] `cargo clippy -- -D warnings`; `cargo fmt -- --check` exit 0
- [ ] The version-read / migrate / stamp sequence in `initialize` runs inside a single `TransactionBehavior::Immediate` transaction, and the `journal_mode`/`synchronous`/`busy_timeout` pragmas remain OUTSIDE it (read the code — `grep -c 'TransactionBehavior::Immediate' src/db.rs` → 1)
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
- Reviewer should also scrutinize Step 1: that the transaction wraps ONLY the version-read / migrate / stamp sequence and that no `PRAGMA journal_mode` call was pulled inside it (SQLite cannot change journal mode inside a transaction — the symptom would be plan 004's `wal_mode_enabled_for_file_database` failing, or WAL silently not engaging).
- Once Step 1 lands, any future schema migration added under `version < SCHEMA_VERSION` is automatically serialized — keep new migration work inside that transaction rather than adding a second one.

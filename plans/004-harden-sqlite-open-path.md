# Plan 004: Harden the SQLite open path — busy timeout, WAL, versioned migration, honest errors

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 7a8b4ca..HEAD -- src/db.rs`
> If `src/db.rs` changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: S
- **Risk**: LOW (WAL adds `-wal`/`-shm` sidecar files next to `commits.db`; harmless for a local single-user DB)
- **Depends on**: none
- **Category**: bug
- **Planned at**: commit `7a8b4ca`, 2026-09-05
- **Revised**: 2026-09-05 at commit `fcb3157` — Step 1 corrected. The original set
  `synchronous=NORMAL` unconditionally while tolerating a failed WAL switch; `NORMAL` is only
  corruption-safe *in WAL mode*, so that combination introduced a small corruption window on
  filesystems where WAL is unavailable. `synchronous` is now gated on WAL actually engaging,
  read back via `pragma_update_and_check`. Added test 6 to prove the pairing on a real file DB.

## Why this matters

`diddo hook` runs on **every git commit** and opens the SQLite database each time. Four defects in that path:

1. **No busy timeout**: SQLite's default busy handler returns `SQLITE_BUSY` immediately. Two commits landing at the same moment (parallel repos, scripted batches, rebases) make one `diddo hook` fail — that commit is never recorded, silently from the user's perspective (the git commit itself succeeds; an error line prints after it).
2. **Rollback journal + `synchronous=FULL`** on every insert: several fsyncs plus journal create/delete per commit — the dominant I/O cost of the hook.
3. **The schema batch and a `PRAGMA table_info` migration probe run on every open**, forever, even though the migration can apply at most once.
4. **A filesystem error is reported as an SQL error**: `fs::create_dir_all` failure is wrapped in `rusqlite::Error::ToSqlConversionFailure`, so a permissions/read-only-FS problem prints as a type-conversion failure. And a nonexistent local midnight (DST spring-forward at 00:00 — real in `America/Santiago`, `America/Havana`, `Asia/Beirut`) makes every summary abort with the baffling message `Query is not read-only` (the Display of `rusqlite::Error::InvalidQuery`).

## Current state

- `src/db.rs` — all persistence. Key excerpts at `7a8b4ca`:

`Database::open` (lines 54–67):

```rust
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
            fs::create_dir_all(parent)
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        }
        let connection = Connection::open(path)?;
        Self::initialize(connection)
    }
```

`initialize` (lines 74–79):

```rust
    fn initialize(connection: Connection) -> Result<Self> {
        connection.execute_batch(SCHEMA)?;
        run_author_email_migration(&connection)?;
        Ok(Self { connection })
    }
```

`SCHEMA` (lines ~22–45): one batch of `CREATE TABLE IF NOT EXISTS commits`, `CREATE UNIQUE INDEX IF NOT EXISTS ... ON commits (repo_path, hash)`, `CREATE TABLE IF NOT EXISTS ai_summary_cache`.

`run_author_email_migration` (lines 205–210): checks `PRAGMA table_info(commits)` for `author_email`, `ALTER TABLE` if missing.

`local_day_start_in_utc` (lines 231–243):

```rust
    let datetime = match timezone.from_local_datetime(&local_midnight) {
        LocalResult::Single(value) => value,
        LocalResult::Ambiguous(first, _) => first,
        LocalResult::None => return Err(rusqlite::Error::InvalidQuery),
    };
```

- `db::Result` is `rusqlite::Result` (alias near the top of the file). Callers (`src/main.rs:350,500`, `src/interactive.rs:414-422`) all `?` into `Box<dyn Error>`, so keeping `rusqlite::Error` as the error type (with better variants/messages) requires **no** caller changes — this plan deliberately keeps all public signatures stable.
- There is no `journal_mode`, `synchronous`, `busy_timeout`, or `user_version` anywhere in `src/` (verify: `grep -rn 'busy_timeout\|journal_mode\|user_version' src/` → empty).
- Test conventions: `Database::open_in_memory()` in tests; inline `#[cfg(test)] mod tests` at the bottom of `db.rs` with ~18 tests.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| DB tests | `cargo test db::` | all pass |
| Full suite | `cargo test` | 0 failed |
| Lint/format | `cargo clippy -- -D warnings && cargo fmt -- --check` | exit 0 |

## Scope

**In scope**:
- `src/db.rs` only (plus its inline tests)

**Out of scope**:
- Changing any public function signature or the `db::Result` alias — callers must not need edits.
- Aggregation queries for the activity report, hook.rs subprocess changes (other findings/plans).
- Deleting the legacy `commit_table_column_names` helper.

## Git workflow

- Branch: `advisor/004-harden-sqlite-open-path`
- Commit style: conventional commits, e.g. `fix: add busy timeout and WAL to hook database writes`. No AI attribution.
- Do NOT push or open a PR unless instructed.

## Steps

### Step 1: Pragmas on every connection

In `initialize`, before the schema work:

```rust
    fn initialize(connection: Connection) -> Result<Self> {
        connection.busy_timeout(std::time::Duration::from_secs(5))?;

        // WAL lets a reader coexist with the hook's writer and removes the
        // rollback-journal create/delete churn on every commit. It can legitimately
        // fail on filesystems without shared memory (NFS/SMB home directories), and
        // on in-memory databases it simply reports "memory" — diddo must keep working
        // in both cases, so a non-WAL outcome is tolerated rather than fatal.
        let journal_mode = connection
            .pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get::<_, String>(0))
            .unwrap_or_default();

        // synchronous=NORMAL is corruption-safe ONLY in WAL mode. SQLite documents a
        // small but non-zero chance of corruption on power loss when NORMAL is combined
        // with a rollback journal, so if WAL did not take effect leave synchronous at
        // its default (FULL) and accept the slower writes.
        if journal_mode.eq_ignore_ascii_case("wal") {
            connection.pragma_update(None, "synchronous", "NORMAL")?;
        }
        ...
```

**Do not set `synchronous=NORMAL` unconditionally.** The two pragmas are a package deal:
`NORMAL` is only safe because WAL makes it safe. Setting `NORMAL` while tolerating a failed
WAL switch leaves a corruption window on exactly the filesystems where WAL is unavailable —
which is the opposite of hardening. Read the mode back and gate on it, as above.

Use `pragma_update_and_check` for `journal_mode` (it wraps `query_row`, and `journal_mode`
returns the resulting mode as a row). Note for accuracy: in rusqlite 0.38 plain `pragma_update`
is implemented with `execute_batch` and so would *not* error on a row-returning pragma — but it
also discards the result, which is the value you need here. `busy_timeout` must propagate its
error.

**Verify**: `cargo test db::` → all pass (in-memory DBs tolerate the pragmas).

### Step 2: Gate schema + migration behind `user_version`

Replace the unconditional schema/migration with:

```rust
        const SCHEMA_VERSION: i64 = 1;
        let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version < SCHEMA_VERSION {
            connection.execute_batch(SCHEMA)?;
            run_author_email_migration(&connection)?;
            connection.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        } else if version > SCHEMA_VERSION {
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_MISMATCH),
                Some(format!(
                    "database schema version {version} is newer than this diddo build supports ({SCHEMA_VERSION}); upgrade diddo (run `diddo update`)"
                )),
            ));
        }
```

Existing databases have `user_version = 0` and a full schema — the `IF NOT EXISTS` DDL and the idempotent migration handle that first post-upgrade open, then stamp `1` and never probe again. If `rusqlite::ffi` is not accessible with the current feature set, use any constructor that yields a `rusqlite::Error` carrying the message string (e.g. `rusqlite::Error::SqliteFailure` with `ffi::Error::new(1)`); the message is the load-bearing part.

**Verify**: `cargo test db::` passes; add the Step 5 tests before final verification.

### Step 3: Honest error for an uncreatable data directory

Replace the `ToSqlConversionFailure` mapping in `open` with a message-carrying error:

```rust
            fs::create_dir_all(parent).map_err(|error| {
                rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CANTOPEN),
                    Some(format!(
                        "could not create data directory {}: {error}",
                        parent.display()
                    )),
                )
            })?;
```

**Verify**: `cargo build` exits 0.

### Step 4: Survive a nonexistent local midnight

In `local_day_start_in_utc`, replace the `LocalResult::None` arm: walk forward in 1-hour steps (up to 3) until the local time exists — a spring-forward gap is at most 2 hours anywhere on Earth:

```rust
        LocalResult::None => {
            let mut candidate = None;
            for hour in 1..=3u32 {
                if let Some(t) = date.and_hms_opt(hour, 0, 0)
                    && let LocalResult::Single(v) | LocalResult::Ambiguous(v, _) =
                        timezone.from_local_datetime(&t)
                {
                    candidate = Some(v);
                    break;
                }
            }
            candidate.ok_or(rusqlite::Error::InvalidQuery)?
        }
```

(Adapt to compile cleanly; the shape — try 01:00, 02:00, 03:00, take the first that exists — is the requirement. Let-chains are fine: this crate is edition 2024 and already uses them, e.g. `src/main.rs:327-328`.)

**Verify**: `cargo build` exits 0.

### Step 5: Tests

Add to `db.rs`'s test module (model after existing tests there):

1. `schema_version_is_stamped_after_open` — `open_in_memory()`, then `PRAGMA user_version` = 1 (query via `db` needs access to the connection: add a `#[cfg(test)]` accessor or run the query through a small test-only method, matching how other tests reach internals).
2. `open_upgrades_version_zero_database` — create a `Connection::open_in_memory()`, run the old `SCHEMA` batch manually, leave `user_version` 0, then pass it through `initialize` (make `initialize` reachable from tests — it is a private fn in the same module, so tests can call it): expect Ok and version 1.
3. `open_refuses_newer_schema_version` — set `PRAGMA user_version = 99` on a raw connection, run `initialize`, expect `Err` whose Display contains `newer than this diddo build`.
4. `nonexistent_local_midnight_falls_forward` — call `date_range_bounds_in_timezone` with a hand-built `TimeZone` impl whose `from_local_datetime` returns `LocalResult::None` for 00:00 and `Single` for 01:00. Implementing `chrono::TimeZone` is verbose; if it exceeds ~50 lines, an acceptable fallback is to extract the `LocalResult` match into a helper `fn resolve_local_midnight(...)` taking the `LocalResult` directly, and unit-test that helper with constructed `LocalResult` values. Choose whichever is less code.
5. `busy_timeout_is_set` — assert `PRAGMA busy_timeout` returns `5000` on an opened DB. Do NOT write a timing-based contention test; it would be flaky. Keep it deterministic.
6. `wal_mode_enabled_for_file_database` — open a `Database::open()` on a unique path under `std::env::temp_dir()`, assert `PRAGMA journal_mode` is `wal` and `PRAGMA synchronous` is `1` (NORMAL). Then remove the DB **and its `-wal`/`-shm` sidecars** at test end. This is the test that proves the Step 1 pairing actually engages on a real file — the in-memory tests cannot show it, since in-memory databases report `memory` and correctly keep `synchronous` at its default.

**Verify**: `cargo test db::` → all pass, including the 6 new tests. `cargo test` → 0 failed.

## Test plan

Steps 5.1–5.6 above. No existing test should need modification (in-memory open keeps working). Full-suite gate: `cargo test` 0 failed.

## Done criteria

- [ ] `cargo test` → 0 failed, ≥6 new db tests present
- [ ] `synchronous=NORMAL` is set only inside a branch conditional on WAL being active (grep the code and read it — an unconditional `pragma_update(None, "synchronous", ...)` fails this criterion)
- [ ] `cargo clippy -- -D warnings` and `cargo fmt -- --check` exit 0
- [ ] `grep -c 'ToSqlConversionFailure' src/db.rs` → 0
- [ ] `grep -c 'user_version' src/db.rs` ≥ 2
- [ ] `grep -c 'busy_timeout' src/db.rs` ≥ 1
- [ ] No file outside `src/db.rs` (+ `plans/README.md`) modified
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back if:

- `rusqlite::ffi` is not importable with the crate's feature set AND no alternative `rusqlite::Error` constructor can carry a message — report rather than inventing an error-type refactor (signature stability is a hard requirement of this plan).
- Any existing db test fails after Step 1 (would mean pragmas break in-memory mode in this rusqlite version).
- `pragma_update(None, "user_version", ...)` rejects the value type — use `execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))` as the fallback and note it.

## Maintenance notes

- Future schema changes: bump `SCHEMA_VERSION`, add a gated migration step in the `version < SCHEMA_VERSION` branch; never add another unconditional `table_info` probe.
- WAL sidecar files (`commits.db-wal`, `commits.db-shm`) now appear next to the DB; if a future `uninstall`/export feature copies or deletes the DB file, it must handle all three files.
- Reviewer should scrutinize: that a non-WAL `journal_mode` outcome is tolerated, that `busy_timeout` failures propagate, that `synchronous=NORMAL` is applied ONLY when WAL actually engaged (never unconditionally), and that the DST fallback picks the earliest existing hour.
- `rusqlite::ffi` is confirmed importable at rusqlite 0.38 with the `bundled` feature (`pub use libsqlite3_sys as ffi`), and `ffi::Error::new`, `SQLITE_MISMATCH`, `SQLITE_CANTOPEN` all exist — the corresponding STOP condition should not trigger.
- Deferred deliberately: a proper `DbError` enum (would ripple through every caller); reconsider if db error handling grows again.

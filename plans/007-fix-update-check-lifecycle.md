# Plan 007: Make the background update check actually persist its cache (and stop spamming GitHub)

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 7a8b4ca..HEAD -- src/update.rs src/main.rs`
> If `check_for_update`, `spawn_update_check`, or `run_cli` changed since this
> plan was written, compare the excerpts below before proceeding; on a
> mismatch, STOP.

## Status

- **Priority**: P2
- **Effort**: S
- **Risk**: LOW
- **Depends on**: none
- **Category**: bug
- **Planned at**: commit `7a8b4ca`, 2026-09-05
- **Revised**: 2026-09-06 at commit `ed793f0` — the notice wait is now **1000 ms**, not 300 ms
  (maintainer decision). At 300 ms the plan did not achieve its own stated goal: a fetch slower
  than the wait is killed with the process, its answer is never cached, and the throttle record
  written in Step 1 then masks the notice for the full 2 h TTL — so on a slow-ish connection the
  notice would still never appear. Corrected the maintenance note, which wrongly claimed slow
  fetches "surface on the next invocation".

## Why this matters

`run_cli` spawns a background thread running `update::check_for_update`, runs the command, then waits only **50 ms** for the result before `main` returns and the process exits — killing the thread. The cache file is written *inside* that thread, *after* a network round-trip with a 5-second timeout. So for every fast command (`config`, `metadata`, `--raw`, `--table`, any cache-hit summary) the fetch is killed mid-flight and `update_check.json` is never written — which means the 2-hour TTL never engages, and **every** such invocation fires a fresh unauthenticated request at `api.github.com` (rate limit: 60/hour/IP). The "Update available" notice is also almost never printed, because the answer virtually never arrives within 50 ms.

The fix has three parts: (1) write a throttle record to the cache file *before* spawning the fetch, so even a killed fetch doesn't retry for the TTL window; (2) write the cache atomically (temp file + rename) so a killed thread can't leave a corrupt half-written file; (3) give the notice a realistic (but still snappy) wait. As a rider in the same file: `diddo update` currently reports "already up to date" when a release tag fails to parse (`is_newer` returns `false` for any unparseable input — and the repo actually has a historic malformed tag `vv0.4.0`); surface that case honestly.

## Current state

- `src/main.rs:286-307` — `spawn_update_check`: skips `Hook`/`Update` commands, loads config, honors `update.auto_check`, then:

```rust
    let cache_path = paths.update_cache_path;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(update::check_for_update(&cache_path));
    });
    Some(rx)
```

- `src/main.rs:327-334` — after the command runs:

```rust
    if let Some(rx) = update_handle
        && let Ok(Some(latest)) = rx.recv_timeout(StdDuration::from_millis(50))
    {
        eprintln!("Update available: {} → {latest}, run `diddo update`", env!("CARGO_PKG_VERSION"));
    }
```

- `src/update.rs:112-160` — `check_for_update(cache_path)`: reads the cache; if fresh (< `CACHE_TTL_SECS = 2 * 60 * 60`) returns from cache; else calls `fetch_latest_release_tag()` (5 s timeout, `src/update.rs:87-99`); on fetch error writes a "negative cache" (`latest_version: current`); on success writes the real result. Both writes use plain `fs::write`.

- `src/update.rs:70-82`:

```rust
fn strip_v(s: &str) -> &str {
    s.strip_prefix('v').unwrap_or(s).trim()
}

pub fn is_newer(current: &str, latest: &str) -> bool {
    let cur = Version::parse(strip_v(current)).ok();
    let lat = Version::parse(strip_v(latest)).ok();
    match (cur, lat) {
        (Some(c), Some(l)) => l > c,
        _ => false,
    }
}
```

- `src/update.rs` `run` (~lines 183–241): `let latest = fetch_latest_release_tag()?;` then `if !is_newer(current, &latest) { println!("diddo is already up to date ({current})."); return Ok(()); }`.

- Existing tests (update.rs test module, 11 tests): `check_for_update_returns_none_when_cache_has_current_version` and `..._returns_some_when_cache_has_newer_version` write a cache file into a temp path and call `check_for_update` — the fetch path is not exercised (would hit the network). `UpdateCache { latest_version, checked_at }` is serde JSON.

- Conventions: DI seams elsewhere are `_with` variants taking closures (`hook::run_with`, `init::install_with`). Follow that naming.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Targeted tests | `cargo test update::` | all pass |
| Full suite | `cargo test` | 0 failed |
| Lint/format | `cargo clippy -- -D warnings && cargo fmt -- --check` | exit 0 |

## Scope

**In scope**:
- `src/update.rs` (+ its tests)
- `src/main.rs` — `spawn_update_check` and the `recv_timeout` block only

**Out of scope**:
- `self_update` crate usage / the Homebrew vs GitHub `run` flow (except the unparseable-tag message in Step 4).
- `update.auto_check` config semantics; README (already documents the option? — no, that's a docs finding, not this plan).
- Checksum verification of updates (deferred with Plan 005's note).

## Git workflow

- Branch: `advisor/007-fix-update-check-lifecycle`
- Commit style: conventional commits, e.g. `fix: persist update-check cache before fetching and write it atomically`. No AI attribution.
- Do NOT push or open a PR unless instructed.

## Steps

### Step 1: Add a testable seam and atomic writes in update.rs

1. Extract the cache write into a helper and make it atomic:

```rust
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
```

2. Restructure `check_for_update` into a `_with` seam (keep the public fn as a thin wrapper):

```rust
pub fn check_for_update(cache_path: &Path) -> Option<String> {
    check_for_update_with(cache_path, fetch_latest_release_tag)
}

fn check_for_update_with<F>(cache_path: &Path, fetch: F) -> Option<String>
where
    F: FnOnce() -> Result<String, Box<dyn Error>>,
{
    // 1. fresh cache -> answer from cache (unchanged logic)
    // 2. stale/missing -> FIRST write a throttle record:
    //      write_cache(cache_path, &UpdateCache { latest_version: current.into(), checked_at: now });
    //    so a killed process cannot retry before TTL.
    // 3. fetch(); on success overwrite with the real result via write_cache and
    //    return is_newer(current, &latest).then_some(latest);
    //    on error: leave the throttle record in place and return None
    //    (this replaces the old error-path negative-cache write — same effect, now crash-safe).
}
```

Keep `CACHE_TTL_SECS`, `UpdateCache`, and the fresh-cache logic byte-for-byte where possible; the two existing cache tests must keep passing unchanged.

**Verify**: `cargo test update::` → all pass.

### Step 2: Wire the pre-throttle guarantee (already done by Step 1's ordering) and widen the notice window

In `src/main.rs:328`, change `StdDuration::from_millis(50)` → `StdDuration::from_millis(1000)`.

Rationale, and why 1000 rather than a snappier number: the wait costs nothing on the common
path, because a fresh cache answers from a file read in microseconds and `recv_timeout` returns
immediately. It is only consumed when a network fetch is genuinely in flight, which is at most
once per `CACHE_TTL_SECS` (2 h) window.

The value has to exceed typical GitHub API latency or the whole feature stays broken. If the
process exits before the fetch returns, the thread is killed and the real answer is **never
written to the cache** — while the throttle record written in Step 1 says `latest_version =
current`, which suppresses the notice for the full TTL. So with too short a wait, a slow-ish
connection means the user never sees an update notice at all: the fetch is always discarded and
always masked. One second catches the large majority of round-trips, and once a fetch does
complete, the real answer is cached and the notice then shows on every invocation for 2 hours.

Worst case is a ~1 s pause on a single non-hook command once per 2 h window (this is also what
an offline user pays, since `fetch_latest_release_tag` has its own 5 s timeout). `diddo hook`
is unaffected — `spawn_update_check` skips `Hook` and `Update` commands entirely, so the
per-commit path never waits. **This value was chosen by the maintainer; do not "optimize" it
downward.**

Add a one-line comment stating the invariant: "the thread throttles itself via the cache file
before fetching; killing it early is safe (but loses this round's answer)."

**Verify**: `cargo build` exits 0.

### Step 3: New tests for the lifecycle

In `src/update.rs`'s test module (model after the two existing `check_for_update_*` tests — they build a temp cache path):

1. `stale_cache_triggers_fetch_and_stores_result` — write a cache with `checked_at` older than TTL and an old version; call `check_for_update_with(path, || Ok("9.9.9".into()))`; expect `Some("9.9.9")` and the file now contains `latest_version: "9.9.9"`.
2. `throttle_record_written_before_fetch` — call `check_for_update_with(path, || { /* read the cache file HERE, inside the closure */ ... Err("net down".into()) })`; assert the file already existed with `latest_version == CARGO_PKG_VERSION` at fetch time (capture via a `RefCell`/`Cell` or read inside the closure and return the observation through a captured `Arc<Mutex<Option<String>>>`).
3. `fetch_error_leaves_throttle_record` — after (2), a second immediate call with a panicking fetch closure (`|| panic!("must not fetch")` — wrap in `std::panic::catch_unwind`? No: simpler, use a closure that returns Err and assert via a flag that it was NOT invoked) returns `None` without invoking fetch (fresh throttle record suppresses it).
4. `cache_write_is_atomic` — assert no `.json.tmp` file remains next to the cache after a successful check.

**Verify**: `cargo test update::` → all pass including 4 new tests.

### Step 4: Honest message for unparseable release tags in `diddo update`

1. Make `strip_v` strip repeated prefixes: `s.trim_start_matches('v').trim()` (handles the historic `vv0.4.0` shape).
2. Add `fn compare_versions(current: &str, latest: &str) -> Option<bool>` returning `None` when either side fails `Version::parse`, `Some(l > c)` otherwise. Reimplement `is_newer` as `compare_versions(...).unwrap_or(false)` so all silent callers keep their behavior.
3. In `run` (~line 189), replace the `!is_newer(...)` early-return with a match on `compare_versions(current, &latest)`: `None` → print `warning: could not compare versions: release tag '{latest}' is not valid semver` to stderr and return an Err (exit 1); `Some(false)` → the existing "already up to date" message; `Some(true)` → proceed.
4. Tests: `strip_v` on `"vv0.4.0"` → `"0.4.0"`; `compare_versions("0.6.7", "not-a-version")` → `None`. Check the existing `is_newer_strips_v_prefix` test still passes.

**Verify**: `cargo test update::` → all pass. `cargo test` → 0 failed.

## Test plan

Steps 3 and 4 add ~6 tests. Manual sanity (optional, network required): `rm <update_cache_path> && cargo run --quiet -- config && sleep 1 && cat <update_cache_path>` — the cache file should exist with a `checked_at` (get the path from `cargo run --quiet -- config`; the update cache lives next to the config/db — find the exact filename via `grep -n update_cache_path src/paths.rs`).

## Done criteria

- [ ] `cargo test` 0 failed; ≥6 new/updated update tests
- [ ] `cargo clippy -- -D warnings` and `cargo fmt -- --check` exit 0
- [ ] `grep -c 'from_millis(1000)' src/main.rs` → 1 and `grep -c 'from_millis(50)' src/main.rs` → 0 (note: an unrelated `from_millis(80)` spinner tick exists elsewhere in the file — leave it alone)
- [ ] `grep -c 'rename' src/update.rs` ≥ 1 (atomic write in place)
- [ ] `git diff --name-only` ⊆ {src/update.rs, src/main.rs, plans/README.md}

## STOP conditions

Stop and report back if:

- `check_for_update` or `run_cli` no longer match the excerpts.
- The existing two cache tests cannot pass unchanged with the restructure (they define the fresh-cache contract; changing them means the restructure altered semantics).
- You find yourself adding a network call to any test — tests must never hit the network; use the `_with` seam.

## Maintenance notes

- The throttle-before-fetch pattern deliberately conflates "we could not find out" with "you are up to date": a killed fetch leaves `latest_version = current`, so the notice is suppressed until the TTL expires. That is the price of not spamming GitHub, and it is why the wait must be long enough to usually catch the fetch (see Step 2). A future refinement, if this ever matters, is to split the record into `last_attempt_at` (throttle) and an optional `latest_version` (last known answer) so the two states are distinguishable — deliberately not done here.
- If a future change adds authenticated GitHub requests, the TTL/throttle logic is the single place rate policy lives.
- Reviewer should scrutinize: that the error path no longer double-writes (old code wrote the negative cache in the error arm; new code relies on the pre-written throttle record).

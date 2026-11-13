# Plan 009: Delete the test-only AI shadow path — test the real one, deduplicate it, and read the cache first

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 7a8b4ca..HEAD -- src/main.rs src/ai/mod.rs`
> Plan 006 may have touched `parse_cli` in src/main.rs — that drift is fine.
> If `render_summary_output`, `try_ai_summary`, or `src/ai/mod.rs` changed,
> compare the excerpts below before proceeding; on a mismatch, STOP.

## Status

- **Priority**: P2
- **Effort**: M
- **Risk**: LOW
- **Depends on**: none (but plan 010 depends on THIS — land 009 first)
- **Category**: tests + tech-debt
- **Planned at**: commit `7a8b4ca`, 2026-09-05

## Why this matters

The tool's flagship behavior — try AI, degrade gracefully to raw output with a warning — is effectively untested and duplicated:

1. `src/main.rs:676-718` defines `try_ai_summary` annotated `#[cfg(test)]`. It is **compiled out of the release binary** and called only by three tests. The shipped logic lives separately, inline in `render_summary_output`, as a *different implementation* that happens to produce the same warning strings. The tests pass against dead code; a regression in the real per-profile loop fails nothing.
2. Inside `render_summary_output` (`src/main.rs:806-858`), the summarize-then-cache block is duplicated **verbatim** across the `show_indicator` (spinner) branch and the plain-stderr branch — only the progress-bar wrapper differs. Any change must be made twice or the branches drift.
3. The provider is constructed *before* the cache is consulted, and `detect_installed_tools()` walks the whole `PATH` twice per run (once in `primary_provider_identity`, once in `create_provider`). Worse: if provider construction fails (CLI uninstalled, API key removed), the function returns raw output **without ever reading the cache** — previously cached summaries become unreachable exactly when the provider breaks.

## Current state

- `src/main.rs:676-718` — `#[cfg(test)] fn try_ai_summary<F>(...) -> AiSummaryAttempt`: builds a provider via the injected closure, returns `AiSummaryAttempt { summary, warning }` with the strings `"AI summary unavailable: {error}. Falling back to raw output."` / `"AI summary failed: {error}. Falling back to raw output."`.

- `src/main.rs:720-905` — `render_summary_output(database, summary_args, window, commits, load_config, create_provider)`. Skeleton of the AI branch (lines 751–864):

```rust
    let (_, warning) = if should_try_ai_summary(summary_args) {
        let config = load_config()?;
        let instructions = config.ai.resolved_prompt_instructions();
        let provider_identity = ai::primary_provider_identity(&config.ai).ok();
        let provider = match create_provider(&config.ai) {
            Ok(p) => p,
            Err(error) => {
                // renders raw output and RETURNS EARLY — never reads the cache
                ...
            }
        };
        let show_indicator = std::io::stderr().is_terminal();
        let mut warnings = Vec::new();
        for profile_group in groups.iter_mut() {
            let profile_commits: Vec<db::Commit> = ...;
            let prompt = ai::build_prompt(&profile_commits, period, instructions);
            let cache_key_opt = provider_identity.as_ref().map(|(provider_id, model_id)| {
                compute_cache_key(provider_id, model_id, period, &profile_group.profile_label, &prompt)
            });
            let cached = match (cache_key_opt.as_deref(), summary_args.no_cache) {
                (Some(key), false) => database.get_cached_summary(key).ok().flatten(),
                _ => None,
            };
            let summary = if let Some(cached_summary) = cached {
                Some(cached_summary)
            } else if show_indicator {
                // spinner setup ... then:
                let attempt = match provider.summarize(&profile_commits, period) { ... };
                // warnings.push / set_cached_summary — DUPLICATED BLOCK A
            } else {
                eprintln!("Generating AI summary...");
                let attempt = match provider.summarize(&profile_commits, period) { ... };
                // warnings.push / set_cached_summary — DUPLICATED BLOCK B (verbatim copy of A)
            };
            profile_group.ai_summary = summary; // via if let Some(s)
        }
        ...
```

- The three tests to re-point (in `mod tests`, `src/main.rs`): `ai_attempt_keeps_json_mode_eligible_for_ai_summary` (~line 1618), `ai_attempt_surfaces_warning_when_provider_is_unavailable` (~line 1645), `ai_attempt_surfaces_warning_when_provider_fails_at_runtime`. They use existing test helpers `sample_commit(...)` and provider stubs like `SuccessProvider(String)` (search `struct SuccessProvider` in the test module) and inject `|_config| Ok(Box::new(...))` / `Err(AiError::new(...))` closures — the same closure shape `render_summary_output` accepts, so re-pointing is mechanical.

- `src/ai/mod.rs:97-112` — `create_provider(config)` calls `select_provider(config, &cli_provider::detect_installed_tools())`; `src/ai/mod.rs:205-225` — `primary_provider_identity(config)` calls the same pair. `detect_installed_tools` (`src/ai/cli_provider.rs:100-110`) stats every PATH dir for 4 binaries.

- `render_summary_output` already takes `database: &db::Database`; tests can pass `db::Database::open_in_memory().unwrap()`.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Targeted tests | `cargo test tests::` (main.rs module) | all pass |
| Full suite | `cargo test` | 0 failed |
| Lint/format | `cargo clippy -- -D warnings && cargo fmt -- --check` | exit 0 |
| Dead code gone | `grep -c 'try_ai_summary' src/main.rs` | see done criteria |

## Scope

**In scope**:
- `src/main.rs` — `render_summary_output`, deletion of `try_ai_summary`, the three tests + any new tests
- `src/ai/mod.rs` — add `_with` variants taking pre-detected tools (non-breaking additions)

**Out of scope**:
- Changing WHICH provider identity the cache key uses (that is Plan 010 — here the key logic stays byte-identical, only its position moves).
- `AiProvider` trait signature, `FallbackProvider` internals.
- Timeouts / max_tokens (Plan 011).
- Decomposing main.rs into modules.

## Git workflow

- Branch: `advisor/009-test-real-ai-fallback-path`
- Commit style: conventional commits, e.g. `refactor: deduplicate AI summary path and test the shipped implementation`. No AI attribution.
- Do NOT push or open a PR unless instructed.

## Steps

### Step 1: Single PATH scan

In `src/ai/mod.rs`, add non-breaking variants and keep the originals as wrappers:

```rust
pub fn create_provider_with(config: &AiConfig, tools: &[CliTool]) -> Result<Box<dyn AiProvider>> { /* body of create_provider, using `tools` */ }
pub fn create_provider(config: &AiConfig) -> Result<Box<dyn AiProvider>> {
    create_provider_with(config, &cli_provider::detect_installed_tools())
}
pub fn primary_provider_identity_with(config: &AiConfig, tools: &[CliTool]) -> Result<(String, String)> { ... }
pub fn primary_provider_identity(config: &AiConfig) -> Result<(String, String)> { ... }
```

`CliTool` must be importable from `main.rs` (`ai::cli_provider::CliTool` — check its visibility; it is `pub` in a `pub mod`, adjust `use` as needed). In `render_summary_output`, call `cli_provider::detect_installed_tools()` once and pass it to both `_with` functions. NOTE: `render_summary_output`'s `create_provider` parameter is an injected closure `FnOnce(&config::AiConfig) -> ai::Result<...>` — keep that closure signature unchanged (tests depend on it); the single-scan optimization applies to the *identity* call inside the function plus the default closure passed at the call site in `run_summary_command` (`src/main.rs:512`): change that argument from `ai::create_provider` to a closure capturing the detected tools... which is not possible before the function runs. Simplest correct shape: compute identity inside `render_summary_output` via `primary_provider_identity(&config.ai)` as today, and have `create_provider` remain the injected closure — then achieve the single scan by changing `run_summary_command` to detect tools once and pass BOTH a precomputed closure `move |cfg| ai::create_provider_with(cfg, &tools)`... closures borrowing locals across the call are fine here. If lifetimes fight you (they may, since `create_provider` is `FnOnce` and `tools` is local), an acceptable outcome is: leave the double scan, implement only the cache-first reordering, and note the scan dedup as skipped — it is the least important of the three goals. Do not burn more than one attempt on it.

**Verify**: `cargo build` exits 0.

### Step 2: Extract the duplicated summarize+cache block

In `render_summary_output`, hoist duplicated blocks A and B into a real (NOT `#[cfg(test)]`) helper in `src/main.rs`:

```rust
fn summarize_and_cache(
    provider: &dyn ai::AiProvider,
    profile_commits: &[db::Commit],
    period: &str,
    cache_key: Option<&str>,
    database: &db::Database,
    warnings: &mut Vec<String>,
) -> Option<String> {
    match provider.summarize(profile_commits, period) {
        Ok(s) => {
            if let Some(key) = cache_key {
                let _ = database.set_cached_summary(key, &s);
            }
            Some(s)
        }
        Err(error) => {
            warnings.push(format!("AI summary failed: {error}. Falling back to raw output."));
            None
        }
    }
}
```

The spinner branch becomes: create spinner → `let summary = summarize_and_cache(...)` → `pb.finish_and_clear()` → use summary. The else branch: `eprintln!("Generating AI summary...")` → same call. Warning strings must remain byte-identical (tests assert them).

**Verify**: `cargo test` → the three `ai_attempt_*` tests still pass (they still call `try_ai_summary` at this point), everything else green.

### Step 3: Read the cache before constructing the provider

Reorder inside the AI branch:

1. Compute `provider_identity` (as today).
2. **First loop**: for each profile group, build prompt + cache key, attempt `get_cached_summary` (honoring `no_cache`); store hits in the group, collect misses (indices + prompt/key).
3. If there are no misses → skip provider construction entirely.
4. If there are misses → `create_provider(...)`; on error, render output (groups with cache hits keep their summaries!) with the existing `"AI summary unavailable: ..."` warning; on success, run the miss loop with `summarize_and_cache`.

This changes observable behavior in exactly one intended way: cached summaries render even when the provider is broken/absent, and the "unavailable" warning accompanies them for the missed groups. The existing early-return block (lines 755–781) collapses into the normal render path — the `(combined_summary, warning)` plumbing at the bottom already handles partially-filled groups.

**Verify**: `cargo test` → 0 failed (some render tests may assert the old all-raw-on-provider-error behavior; if a test asserts that a *cache-hit* group renders raw when the provider errors, update it to the new contract and say so in the report; if a test asserts warning text, that text is unchanged).

### Step 4: Delete `try_ai_summary`, re-point its tests

1. Delete `src/main.rs:676-718` (`try_ai_summary`) and its entry in the test module's `use` list (line ~1027).
2. Rewrite the three `ai_attempt_*` tests to call `render_summary_output` directly, e.g.:

```rust
    let database = crate::db::Database::open_in_memory().unwrap();
    let rendered = render_summary_output(
        &database,
        SummaryArgs::default(),
        test_window("today"),           // build a SummaryWindow inline or add a small helper
        vec![sample_commit("aaa1111", "diddo", "/tmp/diddo", 9, 15)],
        || Ok(crate::config::AppConfig::default()),
        |_config| Err(ai::AiError::new("no AI provider configured or detected")),
    )
    .unwrap();
    assert_eq!(
        rendered.warning.as_deref(),
        Some("AI summary unavailable: no AI provider configured or detected. Falling back to raw output.")
    );
    assert!(rendered.output.contains("aaa1111"));  // raw fallback rendered
```

Keep the three test names (drop the now-wrong `ai_attempt_` prefix if you prefer `render_summary_*`; keep intent identical): provider-unavailable → warning + raw output; provider-fails-at-runtime (`SuccessProvider`-style stub returning Err from `summarize`) → `"AI summary failed: ..."` warning; json-mode → output is valid JSON and AI summary text appears in it. A `SummaryWindow` literal needs `from/to/date_label/ai_period/exact_bounds` — mirror how other render tests in the module construct windows (search `SummaryWindow {` in the test module for an existing example to copy).
3. Add one NEW test for the Step 3 contract: seed the cache via `database.set_cached_summary(key, "CACHED")` where `key` is computed with `compute_cache_key` using the identity that `primary_provider_identity` yields for the test config — if that identity errors for a default config (likely: no provider configured), instead drive the test with a config whose `ai.provider`/`api_key` fields are set so identity resolves (`config::AiConfig` fields are public in-crate — construct directly), a failing `create_provider`, and assert the cached text appears in the output alongside the unavailable-warning.

**Verify**: `cargo test` → 0 failed; `grep -c 'try_ai_summary' src/main.rs` → 0 (`should_try_ai_summary` — different symbol — still exists; grep exactly).

## Test plan

Three rewritten tests + one new cache-first test (Step 4), all against the shipped `render_summary_output`. Full gate `cargo test`.

## Done criteria

- [ ] `grep -cw 'try_ai_summary' src/main.rs` → 0 (word-boundary; `should_try_ai_summary` survives)
- [ ] The literal duplicated `set_cached_summary`/`AI summary failed` block appears exactly once in `render_summary_output`'s body (`grep -c 'AI summary failed' src/main.rs` → 1 outside tests)
- [ ] Cache-first: `create_provider` closure invoked only when at least one group misses (asserted by the new test — a provider closure that panics + a fully-cached fixture is the strongest form; use it if convenient)
- [ ] `cargo test` 0 failed; `cargo clippy -- -D warnings`; `cargo fmt -- --check` exit 0
- [ ] `git diff --name-only` ⊆ {src/main.rs, src/ai/mod.rs, plans/README.md}

## STOP conditions

Stop and report back if:

- `render_summary_output` no longer matches the skeleton (drift beyond plans 006/010 touches).
- Rewriting a test requires changing a user-visible warning string.
- The Step 1 lifetime fight consumes more than one serious attempt — skip scan-dedup per the instructions there and continue.
- More than ~6 existing tests break in Step 3.

## Maintenance notes

- Plan 010 (cache key = producing provider) edits `summarize_and_cache` and the identity plumbing — this plan's helper is deliberately shaped so 010 can thread identity through one place.
- The cache-first reorder means `diddo today` can serve stale summaries with a warning when the provider is broken — intended (documented cache has no TTL); reviewer should confirm the warning still surfaces so users know AI didn't run.
- Deferred: main.rs decomposition (separate tech-debt finding) — do not start it here.

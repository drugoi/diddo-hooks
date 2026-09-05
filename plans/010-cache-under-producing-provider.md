# Plan 010: Cache AI summaries under the provider that actually produced them

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 7a8b4ca..HEAD -- src/main.rs src/ai/`
> Plan 009 is a prerequisite and HAS changed these files (deleted
> `try_ai_summary`, added `summarize_and_cache`, cache-first ordering). Verify
> those specific shapes exist before proceeding; if plan 009 has NOT landed,
> STOP — this plan builds on its helper.

## Status

- **Priority**: P2
- **Effort**: M
- **Risk**: LOW (existing cache entries become unreachable under new keys — a one-time cache miss, by design)
- **Depends on**: plans/009-test-real-ai-fallback-path.md
- **Category**: bug (design-decision drift)
- **Planned at**: commit `7a8b4ca`, 2026-09-05

## Why this matters

The cache design doc, `docs/plans/2026-03-10-ai-response-cache-design.md`, line 34, states verbatim:

> **Fallback provider:** When the first provider fails and fallback succeeds, cache only the successful result under the provider/model that produced it.

The code does not do this. `render_summary_output` computes one identity up front — `ai::primary_provider_identity(&config.ai)`, which is the **first candidate** in the fallback chain — and writes every summary under that key. `FallbackProvider::summarize` (`src/ai/mod.rs:239-255`) silently walks its provider list and returns only a `String`, so the caller cannot know which provider succeeded. Concretely: a user with the `claude` CLI installed plus an OpenAI API key gets an OpenAI-generated summary cached under `("claude", "default")` whenever the CLI transiently fails. Because the cache has no TTL (a documented decision), that mislabeled summary is served as "the claude summary" for that commit set forever; `--no-cache` is the only escape.

## Current state

(As of `7a8b4ca`, adjusted by Plan 009 — verify both.)

- `src/ai/mod.rs:56-…` — the trait (check exact line: `grep -n 'trait AiProvider' src/ai/mod.rs`):

```rust
pub trait AiProvider {
    fn summarize(&self, commits: &[Commit], period: &str) -> Result<String>;
}
```

Implementors: `CliProvider` (`src/ai/cli_provider.rs:93-97`), `ApiProvider` (`src/ai/api_provider.rs:105-111`), `FallbackProvider` (`src/ai/mod.rs:239-255`), plus test stubs in `src/main.rs`'s test module (`SuccessProvider`, and whatever failing stubs plan 009 added) and possibly in `src/ai/mod.rs` tests.

- `FallbackProvider { providers: Vec<Box<dyn AiProvider>> }` is built in `create_provider` (`src/ai/mod.rs:97-112`) from `select_provider`'s ordered `Vec<ProviderChoice>` — at construction time the identity of each provider IS known (`ProviderChoice::Cli(tool)` / `ProviderChoice::Api(kind)`), it just isn't retained.

- Identity strings used for cache keys today (`primary_provider_identity`, `src/ai/mod.rs:205-225`): CLI → `(tool.display_name().to_ascii_lowercase(), "default")`; API → `(config.resolved_provider() or kind.display_name(), config.resolved_model() or kind.default_model())`. **These exact strings must be reproduced per-provider** so keys stay stable for the primary-provider case (an existing single-provider user must keep their cache hits).

- `compute_cache_key(provider_id, model_id, period, profile, prompt)` — `src/main.rs:644-662` (SHA-256 over NUL-joined fields). Unchanged by this plan.

- After Plan 009: `summarize_and_cache(provider, profile_commits, period, cache_key, database, warnings)` is the single call site of `provider.summarize`; the cache-read loop tries one key per group.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Full suite | `cargo test` | 0 failed |
| AI tests | `cargo test ai::` | all pass |
| Lint/format | `cargo clippy -- -D warnings && cargo fmt -- --check` | exit 0 |

## Scope

**In scope**:
- `src/ai/mod.rs` — trait extension (default method), `FallbackProvider`, identity plumbing, a new `provider_identities` fn
- `src/main.rs` — `summarize_and_cache` + the cache read/write key logic in `render_summary_output`; test stubs
- `src/ai/cli_provider.rs`, `src/ai/api_provider.rs` — override the new method (small)
- `docs/plans/2026-03-10-ai-response-cache-design.md` — no change needed (code moves toward the doc)

**Out of scope**:
- Cache schema/table changes; `compute_cache_key`'s algorithm.
- Timeouts, max_tokens (Plan 011).
- Any change to which providers are selected or their order.

## Git workflow

- Branch: `advisor/010-cache-under-producing-provider`
- Commit style: conventional commits, e.g. `fix: cache AI summaries under the provider that produced them`. No AI attribution.
- Do NOT push or open a PR unless instructed.

## Steps

### Step 1: Extend the trait with a default identity-carrying method

In `src/ai/mod.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderIdentity {
    pub provider_id: String,
    pub model_id: String,
}

pub trait AiProvider {
    fn summarize(&self, commits: &[Commit], period: &str) -> Result<String>;

    /// Like `summarize`, but reports which provider produced the result.
    /// `None` identity means "unknown" — callers fall back to the primary identity.
    fn summarize_identified(
        &self,
        commits: &[Commit],
        period: &str,
    ) -> Result<(String, Option<ProviderIdentity>)> {
        self.summarize(commits, period).map(|s| (s, None))
    }
}
```

A default method keeps every existing implementor and test stub compiling untouched.

**Verify**: `cargo build` exits 0; `cargo test` 0 failed.

### Step 2: Make the concrete providers self-identify

1. `CliProvider` — override `summarize_identified` returning `Some(ProviderIdentity { provider_id: self.tool.display_name().to_ascii_lowercase(), model_id: "default".into() })`. (`tool` is a private field of the same module's struct — implement inside `cli_provider.rs`.)
2. `ApiProvider` — override returning its kind/model. Careful: the identity strings must match what `primary_provider_identity` produces for the same choice — that function prefers `config.resolved_provider()` (the *config-normalized name*) over `kind.display_name()`. `ApiProvider` stores `kind` and `model` (check its fields at `src/ai/api_provider.rs:70-88`); `from_config` has access to the config — capture the resolved provider string at construction into a new private field so the identity is exactly the string `primary_provider_identity` would produce. If `from_config`'s signature makes that awkward, compute `kind.display_name().to_ascii_lowercase()` and ALSO change `primary_provider_identity` to use the same (they only differ if a user writes a differently-cased provider name in config — normalization lowercases already; confirm by reading `normalized_provider` in `src/config.rs:143-148`, which trims + lowercases — so `resolved_provider()` for a supported kind is always exactly `"openai"`/`"anthropic"`, equal to `kind.display_name()` lowercased; verify `ApiKind::display_name` returns those strings, then either source is fine).
3. `FallbackProvider::summarize_identified` — walk providers calling each one's `summarize_identified`, return the first success (with its identity). Reimplement `summarize` as `self.summarize_identified(...).map(|(s, _)| s)` to keep one loop.

**Verify**: `cargo test` 0 failed.

### Step 3: Use the winning identity for cache writes; try all identities for reads

In `src/ai/mod.rs` add (mirroring `primary_provider_identity`, sharing its match arm via a small helper so the two cannot drift):

```rust
pub fn provider_identities(config: &AiConfig) -> Result<Vec<(String, String)>>
```

returning the identity tuple for EVERY choice from `select_provider`, in order. Refactor `primary_provider_identity` to `provider_identities(...)?.into_iter().next()`.

In `src/main.rs` `render_summary_output`:

- **Read path**: for each profile group, compute a candidate key per identity in `provider_identities` order (prompt and profile fixed, identity varies); take the first cache hit. (Design-doc compliant: a summary produced by the fallback is found under the fallback's key.)
- **Write path**: switch `summarize_and_cache` to call `provider.summarize_identified`; compute the write key from the returned identity when `Some`, else from the primary identity (pass both through as parameters — the helper's signature grows to accept the pieces needed to compute a key: period, profile label, prompt, and the identities; or simplest, pass a closure `key_for: impl Fn(&ProviderIdentity) -> String` plus the primary-key fallback).

**Verify**: `cargo test` 0 failed.

### Step 4: Tests

1. In `src/ai/mod.rs` tests: `fallback_reports_identity_of_successful_provider` — a `FallbackProvider` of two stubs (first errors, second succeeds with a known identity via an overridden `summarize_identified`); assert the returned identity is the second's.
2. In `src/main.rs` tests: `fallback_summary_cached_under_producing_provider` — drive `render_summary_output` with a `create_provider` closure returning a stub whose `summarize_identified` yields `("S", Some(identity B))` while the config's primary identity is A; after the call, assert `database.get_cached_summary(key_B)` is `Some("S")` and `key_A` is `None`. Compute `key_A`/`key_B` with `compute_cache_key` in the test using the same prompt (`ai::build_prompt(&commits, "today", None)`) and profile label — copy the label logic from how the code derives `profile_label` (see `summary_group::group_commits_by_profile_then_repo`; for a single-email fixture it is that email — confirm by reading the grouping fn once).
3. `cached_fallback_summary_found_on_read` — seed the cache under identity B's key; provider closure PANICS if constructed (cache-first from plan 009 must satisfy the read without a provider... note: with multiple identities configured, identity resolution must not itself require provider construction — `provider_identities` only inspects config+PATH, fine); assert output contains the cached text.

**Verify**: `cargo test` → all pass including the 3 new tests.

## Test plan

The 3 tests in Step 4; plus the plan-009 tests keep passing unchanged (their stubs use the default `summarize_identified` → `None` → primary-key fallback, preserving old key behavior for single-provider setups).

## Done criteria

- [ ] `cargo test` 0 failed, incl. 3 new tests
- [ ] `cargo clippy -- -D warnings`; `cargo fmt -- --check` exit 0
- [ ] `grep -c 'summarize_identified' src/ai/mod.rs src/ai/cli_provider.rs src/ai/api_provider.rs src/main.rs` — present in all four files
- [ ] Single-provider cache keys unchanged: the plan-009 seeded-cache test still passes without edits
- [ ] `git diff --name-only` ⊆ {src/main.rs, src/ai/mod.rs, src/ai/cli_provider.rs, src/ai/api_provider.rs, plans/README.md}

## STOP conditions

Stop and report back if:

- Plan 009's `summarize_and_cache` helper does not exist (009 not landed or landed differently).
- Preserving byte-identical identity strings for the primary case proves impossible (Step 2.2's verification of `resolved_provider` vs `display_name` fails) — key stability is the plan's compatibility contract; report the discrepancy.
- The trait change breaks an implementor you cannot find (search `impl AiProvider` across `src/` — expect exactly 3 in src + stubs in test modules).

## Maintenance notes

- Read-path cost: N candidate keys per group instead of 1 — N is ≤ 5 (4 CLIs + 1 API); negligible against a SQLite point lookup each.
- Plan 011 changes provider internals (timeouts); it lands after this and must route its error mapping through `summarize_identified`'s single loop in `FallbackProvider`.
- Reviewer should scrutinize: that a `None` identity (default method) falls back to the PRIMARY key, not a panic/skip — test stubs rely on it.
- The design doc's cache section now matches the code; if behavior is intentionally changed again, update the doc in the same PR.

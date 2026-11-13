# Plan 011: Bound every AI call — subprocess deadline, explicit HTTP timeout, honest truncation, prompt size cap

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 7a8b4ca..HEAD -- src/ai/ src/config.rs README.md`
> Plans 009/010 have likely touched src/ai/mod.rs (identity plumbing) — that
> drift is expected. If `run_cli_command`, `ApiProvider::new`, or
> `build_request_body` changed beyond that, compare excerpts; on mismatch, STOP.

## Status

- **Priority**: P2
- **Effort**: M
- **Risk**: MED (subprocess timeout logic is easy to get subtly wrong; follow the poll-loop shape exactly)
- **Depends on**: plans/010-cache-under-producing-provider.md (provider internals; land 009→010→011 in order)
- **Category**: bug
- **Planned at**: commit `7a8b4ca`, 2026-09-05

## Why this matters

Four unbounded/dishonest behaviors in the AI path, all surfacing as a spinner that never ends or a silently-wrong summary:

1. **The CLI subprocess can hang forever**: `command.output()` on the spawned `claude`/`codex`/`opencode`/`cursor-agent` process has no deadline and no kill path. A wedged CLI hangs `diddo today` behind an animated "Generating AI summary..." indefinitely; the fallback chain never advances because the call never returns.
2. **The API client's timeout is implicit and unconfigurable**: `Client::new()` in reqwest 0.13's blocking mode carries a default 30 s total timeout (undocumented in this codebase). A month-scale summary can legitimately exceed it, and the resulting error message gives no hint that a timeout fired or that it could be raised. `Client::new()` also panics (internal `expect`) if TLS init fails, bypassing the graceful-degradation path every other AI error takes.
3. **Anthropic responses are hard-capped at `max_tokens: 400` and truncation is invisible**: multi-repo summaries get cut mid-sentence, `stop_reason` is never inspected, and the truncated text is then written to the no-TTL cache — served forever.
4. **The prompt is passed as a single argv argument** with one line per commit and no cap; Linux's 128 KiB per-argument limit (`MAX_ARG_STRLEN`) turns large periods into an opaque `E2BIG` failure, and the full prompt is visible in `ps` output.

## Current state

- `src/ai/cli_provider.rs:182-219` — `run_cli_command(tool, prompt)`: builds `Command::new(tool.binary_name())` with per-tool args (`claude -p <prompt>`, `codex exec <prompt>`, `opencode run <prompt>`, cursor's form), then `command.output()?`, returning stdout on success / stderr-derived `io::Error` otherwise.
- `src/ai/api_provider.rs:80-88` — `ApiProvider::new(...)` sets `client: Client::new()`. `request_summary` (lines 113–147) posts JSON, checks status, parses body. `build_request_body` (lines 171–198): OpenAI body has no `max_tokens`; Anthropic body has `"max_tokens": 400`. `extract_summary_text` (lines 200–219) reads `choices[0].message.content` / the first `content[].type=="text"` block; `stop_reason` / `finish_reason` never read.
- `src/ai/mod.rs:258-322` — `build_prompt(commits, period, instructions)`: instructions block, then `Period/Commit count`, then one numbered line per commit, no cap. (Plan 016 adds data fencing here — this plan only adds the size cap; coordinate: the cap goes in regardless, fencing is separate.)
- `src/config.rs` — `AiConfig` fields (search `struct AiConfig`): `provider`, `api_key`, `model`, `prompt_instructions`, `cli` (with `prefer`). Serde-deserialized from `config.toml` `[ai]`. Defaults derive. Tests in the same file show the TOML parsing patterns to copy.
- `src/update.rs:88-92` is the repo's exemplar of a correctly built client: `Client::builder().user_agent(...).timeout(Duration::from_secs(5)).build()?`.
- README config table (README.md ~lines 324–334) lists the `ai.*` keys — new keys must be added there.
- After plan 010, providers implement `summarize_identified`; error mapping below flows through the same single loop.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| AI tests | `cargo test ai::` | all pass |
| Config tests | `cargo test config::` | all pass |
| Full suite | `cargo test` | 0 failed |
| Lint/format | `cargo clippy -- -D warnings && cargo fmt -- --check` | exit 0 |

## Scope

**In scope**:
- `src/ai/cli_provider.rs` — subprocess deadline
- `src/ai/api_provider.rs` — client construction, max_tokens, stop_reason
- `src/ai/mod.rs` — prompt size cap only
- `src/config.rs` — two new optional keys: `ai.timeout_secs`, `ai.max_tokens`
- `README.md` — config table rows for the two new keys

**Out of scope**:
- Prompt data fencing / CLI restrictive flags (Plan 016).
- Switching prompt delivery to stdin — investigated and deliberately deferred: it changes the invocation contract of four external tools that cannot be verified in this environment; the size cap below removes the practical failure. Record as deferred in plans/README.md.
- `cursor-agent` naming (Plan 012); do not touch `binary_name`.

## Git workflow

- Branch: `advisor/011-bound-ai-provider-calls`
- Commit style: conventional commits, e.g. `fix: add timeouts to AI providers and surface truncated summaries`. No AI attribution.
- Do NOT push or open a PR unless instructed.

## Steps

### Step 1: Config keys

In `src/config.rs`, add to `AiConfig` (serde `Option`s, defaulting like the neighbors):

```rust
    pub timeout_secs: Option<u64>,   // applies to both CLI subprocess and API request
    pub max_tokens: Option<u32>,     // Anthropic max_tokens; default 1024
```

Add resolved accessors following the existing style (`resolved_model` etc.): `resolved_timeout()` → `Duration::from_secs(self.timeout_secs.unwrap_or(120))` and `resolved_max_tokens()` → `self.max_tokens.unwrap_or(1024)`. Add config tests mirroring the neighboring TOML-parsing tests: keys absent → defaults; keys set → honored.

**Verify**: `cargo test config::` → all pass.

### Step 2: Subprocess deadline in cli_provider

Replace `command.output()` with a spawn + poll loop (no new dependency):

```rust
fn run_with_deadline(mut command: Command, deadline: Duration) -> io::Result<std::process::Output> {
    use std::process::Stdio;
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let start = std::time::Instant::now();
    loop {
        if child.try_wait()?.is_some() {
            return child.wait_with_output();
        }
        if start.elapsed() >= deadline {
            let _ = child.kill();
            let _ = child.wait(); // reap
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("timed out after {}s (set ai.timeout_secs to raise)", deadline.as_secs()),
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
```

Pipe-buffer caveat: a child that fills its stdout pipe while we only `try_wait` will deadlock — but only for outputs larger than the OS pipe buffer (64 KiB). Summaries are far smaller; add a code comment stating this bound. (`wait_with_output` drains after exit.) `stdin(Stdio::null())` also prevents a CLI that unexpectedly prompts from hanging on input — note this in the same comment.

Thread the timeout: `CliProvider` gains a `timeout: Duration` field set from `config.resolved_timeout()` at construction (`create_provider_with` in `src/ai/mod.rs` builds `CliProvider::new(tool, instructions)` — extend `new`). `run_cli_command(tool, prompt)` becomes `run_cli_command(tool, prompt, timeout)`; the injected-runner test seam `summarize_with_runner` keeps its shape (the runner closure signature already abstracts the call — check how tests inject it and keep them compiling).

Timeout maps to the normal error path (`io::Error` → the existing stderr-message wrapping → `AiError`), so `FallbackProvider` advances to the next provider — which is the point.

**Verify**: `cargo test ai::` passes. Add a real (non-mocked) deadline test: build a `Command::new("sleep")` with arg `"5"` via a test-only call to `run_with_deadline(cmd, Duration::from_millis(200))` → `Err` with `ErrorKind::TimedOut`, in well under 5 s. Gate with `#[cfg(unix)]`.

### Step 3: Explicit, non-panicking API client

In `ApiProvider` (`src/ai/api_provider.rs`): store no eager client. Replace the `client: Client::new()` field init with lazy construction inside `summarize`/`request_summary`:

```rust
        let client = Client::builder()
            .timeout(self.timeout)
            .build()
            .map_err(|error| AiError::new(format!("could not build HTTP client: {error}")))?;
```

with `self.timeout` populated from `config.resolved_timeout()` in `from_config`. (Building a blocking client is cheap; one build per summarize call is fine and kills the panic path.) On request errors, reqwest's own message already says "operation timed out" — additionally wrap: if `error.is_timeout()`, produce `AiError::new(format!("request timed out after {}s (set ai.timeout_secs to raise)", ...))`.

**Verify**: `cargo test ai::` passes (existing api tests use the injected-request seam `summarize_with_client`, unaffected by client construction).

### Step 4: Honest max_tokens

1. `build_request_body`: Anthropic `"max_tokens"` from the configured value (thread `max_tokens: u32` into the function — it currently takes `(kind, model, prompt)`; add the parameter and update call sites + tests). Default becomes 1024 via `resolved_max_tokens`.
2. In `request_summary`, after parsing the body: for Anthropic, read `body["stop_reason"]`; if it equals `"max_tokens"`, return `Err(AiError::new("summary was truncated at N tokens; raise ai.max_tokens in config"))` — an error, so the truncated text is neither shown nor cached (the cache-write only happens on `Ok`, see `summarize_and_cache` in src/main.rs). For OpenAI, do the same for `choices[0].finish_reason == "length"` (only reachable if a future body sets a cap; cheap symmetry).
3. Tests (follow the existing `extract_summary_text`/body-building test patterns in the file): body contains configured max_tokens; a fixture response with `"stop_reason": "max_tokens"` yields the truncation error; a `"stop_reason": "end_turn"` fixture still succeeds.

**Verify**: `cargo test ai::` → all pass with new tests.

### Step 5: Prompt size cap in build_prompt

In `src/ai/mod.rs::build_prompt`, cap the commit list: if `commits.len() > 500`, include the first 500 lines and append `"... and {n} more commits omitted from this prompt."`, and adjust nothing else (the `Commit count:` header keeps the TRUE total — the model should know the real count). Constant `const MAX_PROMPT_COMMITS: usize = 500;` with a comment explaining the Linux 128 KiB argv limit (~130 bytes/line × 500 ≈ 65 KiB, safely under). Add a test: 501 commits → prompt contains the omission line and exactly 500 numbered entries; 500 commits → no omission line.

**Verify**: `cargo test ai::` → all pass.

### Step 6: README

Add two rows to the config table (README.md ~line 326): `ai.timeout_secs` — "Timeout for AI CLI/API calls in seconds (default 120)"; `ai.max_tokens` — "Response token cap for direct API providers (default 1024); truncated responses are treated as errors".

**Verify**: `grep -c 'timeout_secs' README.md` ≥ 1.

## Test plan

Steps 1–5 add ~8 tests (config defaults/overrides, unix deadline, truncation error, stop_reason success, body max_tokens, prompt cap ×2). Full gate `cargo test` 0 failed.

## Done criteria

- [ ] `grep -c 'Client::new()' src/ai/api_provider.rs` → 0
- [ ] `grep -c 'try_wait' src/ai/cli_provider.rs` ≥ 1; `grep -c '"max_tokens": 400' src/ai/api_provider.rs` → 0
- [ ] `grep -c 'MAX_PROMPT_COMMITS' src/ai/mod.rs` ≥ 2 (const + use)
- [ ] `cargo test` 0 failed; `cargo clippy -- -D warnings`; `cargo fmt -- --check` exit 0
- [ ] `git diff --name-only` ⊆ {src/ai/cli_provider.rs, src/ai/api_provider.rs, src/ai/mod.rs, src/config.rs, README.md, plans/README.md}

## STOP conditions

Stop and report back if:

- Plans 009/010 have not landed (helper/identity shapes absent) — ordering is mandatory.
- The `summarize_with_runner` / `summarize_with_client` test seams cannot absorb the new parameters without rewriting >2 existing tests' assertions.
- You are tempted to add the `wait-timeout` crate or async runtime — the poll loop is the required approach (no new dependencies).

## Maintenance notes

- The 64 KiB pipe-buffer bound on the poll loop: if a future feature streams large CLI outputs, switch to reader threads then.
- Deferred (recorded in index): stdin prompt delivery; per-CLI restrictive flags arrive with Plan 016.
- Reviewer should scrutinize: the kill-then-reap order in the deadline loop, and that truncation returns `Err` *before* any cache write.

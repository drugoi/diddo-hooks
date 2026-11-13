# Plan 013: Three small correctness fixes — both-keys inference, API-key masking panic, interactive Ctrl+C

> **Executor instructions**: Follow this plan step by step. The three fixes
> are independent — verify each before starting the next. If anything in the
> "STOP conditions" section occurs, stop and report — do not improvise. When
> done, update the status row for this plan in `plans/README.md` — unless a
> reviewer dispatched you and told you they maintain the index.
>
> **Drift check (run first)**: `git diff --stat 7a8b4ca..HEAD -- src/config.rs src/main.rs src/interactive.rs src/ai/mod.rs`
> Plans 006/009/010/011 may have touched main.rs/ai/mod.rs/config.rs — that
> drift is expected. Verify the three specific excerpts below still match; on
> a mismatch for one fix, STOP for that fix only and do the others.

## Status

- **Priority**: P2
- **Effort**: S
- **Risk**: LOW
- **Depends on**: none
- **Category**: bug
- **Planned at**: commit `7a8b4ca`, 2026-09-05

## Why this matters

1. **Both API keys set → "no AI provider configured"**: exporting both `DIDDO_OPENAI_KEY` and `DIDDO_ANTHROPIC_KEY` (a common shell-profile pattern) makes provider inference return `None`, so a user with *two* configured providers and no AI CLI gets `AI summary unavailable: no AI provider configured or detected` — actively misleading, with no hint that setting `ai.provider` resolves it.
2. **`diddo metadata` panics on non-ASCII keys**: `mask_api_key` guards on byte length but slices at fixed byte offsets; a pasted smart-quote or non-breaking space in the key makes the *diagnostic command* abort with a char-boundary panic — exactly when the user is debugging their config.
3. **Ctrl+C does nothing in interactive mode**: raw mode disables terminal SIGINT generation; crossterm delivers Ctrl+C as a key event, and the key handler matches only `KeyCode` (never modifiers), so `Ctrl+C` falls through to `Action::None`. The reflex escape hatch reads as a hung program. Bonus defect in the same function: teardown runs `execute!(stdout, cursor::Show)?` *before* `disable_raw_mode()?` — an error in the first leaves the shell in raw mode.

## Current state

**Fix 1** — `src/config.rs:150-160`:

```rust
fn infer_provider_from_environment() -> Option<String> {
    match (
        read_env("DIDDO_OPENAI_KEY").is_some(),
        read_env("DIDDO_ANTHROPIC_KEY").is_some(),
    ) {
        (true, false) => Some(String::from("openai")),
        (false, true) => Some(String::from("anthropic")),
        _ => None,
    }
}
```

and `src/ai/mod.rs:162-164` (inside `select_provider`, the no-choices case):

```rust
    if choices.is_empty() {
        return Err(AiError::new("no AI provider configured or detected"));
    }
```

`read_env` is a local helper in config.rs (find with `grep -n 'fn read_env' src/config.rs`).

**Fix 2** — `src/main.rs:488-494`:

```rust
fn mask_api_key(key: &str) -> String {
    if key.len() <= 8 {
        "***".to_string()
    } else {
        format!("{}…{}", &key[..4], &key[key.len() - 4..])
    }
}
```

Called from `format_metadata` (`src/main.rs:454-457`) on the resolved API key.

**Fix 3** — `src/interactive.rs`:

- `action_from_key(code: KeyCode, item_count)` (lines 138–151) — no modifier access, `_ => Action::None`.
- The event loop `run_inner` (lines 230–370): reads `Event::Key(key_event)`, filters `KeyEventKind::Press`, then matches per-UI-state, passing only `key_event.code` down.
- `run` teardown (lines 192–196):

```rust
    let mut stdout = io::stdout();
    execute!(stdout, cursor::Show)?;
    terminal::disable_raw_mode()?;
    result
```

- An existing best-effort helper `restore_terminal` (lines 162–170) already does `let _ = execute!(...); let _ = terminal::disable_raw_mode();` — reuse its pattern.
- Imports: check `use crossterm::event::{...}` at the top of the file — you will need `KeyModifiers`.
- Existing key-handling tests: `action_from_key` tests in the module (find with `grep -n 'action_from_key' src/interactive.rs`).

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Targeted tests | `cargo test config:: && cargo test mask_api_key && cargo test interactive::` | all pass |
| Full suite | `cargo test` | 0 failed |
| Lint/format | `cargo clippy -- -D warnings && cargo fmt -- --check` | exit 0 |

## Scope

**In scope**:
- `src/config.rs` (fix 1 detection helper + test)
- `src/ai/mod.rs` (fix 1 error message + test)
- `src/main.rs` (fix 2 + test)
- `src/interactive.rs` (fix 3 + test)
- `README.md` — one sentence for fix 1 behavior (Notes list, ~line 366)

**Out of scope**:
- Choosing a default provider when both keys are set (product decision — the fix is an actionable error, not a silent pick).
- Testing the full raw-mode terminal loop (separate test-coverage finding).
- `parse_cli`/flag handling (Plan 006).

## Git workflow

- Branch: `advisor/013-small-cli-correctness-fixes`
- Commit style: conventional commits; one commit per fix is ideal (`fix: explain provider choice when both API keys are set`, `fix: make api key masking char-safe`, `fix: quit interactive mode on ctrl-c`). No AI attribution.
- Do NOT push or open a PR unless instructed.

## Steps

### Step 1: Actionable error when both env keys are set

In `src/ai/mod.rs`, at the `choices.is_empty()` return in `select_provider`, differentiate the message. The cleanest seam: `config::AiConfig` knows the env — add a helper in config.rs:

```rust
pub fn both_env_keys_set() -> bool {
    read_env("DIDDO_OPENAI_KEY").is_some() && read_env("DIDDO_ANTHROPIC_KEY").is_some()
}
```

and in `select_provider`:

```rust
    if choices.is_empty() {
        if crate::config::both_env_keys_set() {
            return Err(AiError::new(
                "both DIDDO_OPENAI_KEY and DIDDO_ANTHROPIC_KEY are set; set ai.provider = \"openai\" or \"anthropic\" in config.toml to choose",
            ));
        }
        return Err(AiError::new("no AI provider configured or detected"));
    }
```

Testing env-var-dependent code: the existing config tests manipulate env vars (check how — `grep -n 'set_var\|remove_var' src/config.rs`); env mutation is process-global, and the suite runs multi-threaded. Follow whatever serialization pattern the existing env tests use (a mutex/serial pattern or unique var handling). If existing tests do NOT mutate env, make `both_env_keys_set` delegate to an injectable inner `fn both_keys_from(openai: bool, anthropic: bool) -> bool`-style pure check and unit-test the message selection in `select_provider` via a parameter instead — choose the shape that avoids flaky env races. README: add a Notes bullet: "If both DIDDO_OPENAI_KEY and DIDDO_ANTHROPIC_KEY are set, set `ai.provider` explicitly."

**Verify**: `cargo test` → 0 failed, new test(s) pass.

### Step 2: Char-safe masking

Replace `mask_api_key`:

```rust
fn mask_api_key(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() <= 8 {
        "***".to_string()
    } else {
        let head: String = chars[..4].iter().collect();
        let tail: String = chars[chars.len() - 4..].iter().collect();
        format!("{head}…{tail}")
    }
}
```

Tests (next to any existing `mask_api_key` test — search the test module): ASCII key unchanged behavior (`"sk-abcdefgh1234"` → `"sk-a…1234"`), short key → `"***"`, and a key containing multi-byte chars (e.g. `"клюve-secret-käy"` — a synthetic string, NOT a real credential) does not panic and masks to first/last 4 *characters*.

**Verify**: `cargo test mask_api_key` → passes (add the name to the test fns so the filter matches).

### Step 3: Ctrl+C quits; teardown always restores

1. In `run_inner`'s loop, immediately after the `KeyEventKind::Press` filter, add a global interrupt check (before the per-state match):

```rust
            if key_event.modifiers.contains(KeyModifiers::CONTROL)
                && matches!(key_event.code, KeyCode::Char('c') | KeyCode::Char('d'))
            {
                execute!(stdout, terminal::Clear(ClearType::All), cursor::MoveTo(0, 0))?;
                return Ok(None);
            }
```

(Import `KeyModifiers` from `crossterm::event`.) Returning `Ok(None)` matches the existing Quit path (`main.rs:271` treats `Ok(None)` as clean exit).
2. In `run`'s teardown, make restoration unconditional — replace the two `?` lines with:

```rust
    let mut stdout = io::stdout();
    let _ = execute!(stdout, cursor::Show);
    let _ = terminal::disable_raw_mode();
    result
```

3. Testability: the interrupt check lives in the untestable event loop; extract the predicate so it can be unit-tested:

```rust
fn is_interrupt(key_event: &KeyEvent) -> bool { ... }
```

and test it with constructed `KeyEvent`s (crossterm's `KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)`): Ctrl+C → true, Ctrl+D → true, plain `c` → false, Ctrl+X → false. Model placement after the existing pure-helper tests (`action_from_key` etc.).

**Verify**: `cargo test interactive::` → all pass including the new predicate tests. Manual check: `cargo run` in a real terminal, press Ctrl+C at the menu → clean exit, shell echoes normally afterwards (type a character to confirm). Report the manual check's result.

## Test plan

Steps 1–3 add ~7 tests across three files, each colocated with the module's existing tests. Full gate `cargo test` 0 failed.

## Done criteria

- [ ] `cargo test` 0 failed; new tests present for all three fixes
- [ ] `cargo clippy -- -D warnings`; `cargo fmt -- --check` exit 0
- [ ] `grep -c 'both DIDDO_OPENAI_KEY' src/ai/mod.rs` → 1
- [ ] `grep -c 'chars' src/main.rs` includes the new masking impl; no `&key[..4]` byte-slice remains (`grep -c 'key\[..4\]' src/main.rs` → 0)
- [ ] `grep -c 'KeyModifiers::CONTROL' src/interactive.rs` ≥ 1; teardown uses `let _ =` for both restore calls
- [ ] Manual Ctrl+C check reported
- [ ] `git diff --name-only` ⊆ {src/config.rs, src/ai/mod.rs, src/main.rs, src/interactive.rs, README.md, plans/README.md}

## STOP conditions

Stop and report back if:

- Any of the three excerpts no longer matches (apply the per-fix STOP rule from the header).
- Env-var tests prove flaky under the parallel test runner and no existing serialization pattern exists in the repo — switch to the pure-function shape described in Step 1 rather than adding a serial-test dependency.
- Ctrl+C handling requires touching the panic hook or `restore_terminal` beyond the described teardown change.

## Maintenance notes

- If interactive mode later gains text input fields beyond the range form, Ctrl+C-as-quit must be re-examined (users may expect it to clear the field first) — the extracted `is_interrupt` predicate is the single place to adjust.
- Fix 1 deliberately refuses to auto-pick a provider; if a default is ever chosen, update the README Notes bullet and the error text together.
- Reviewer should scrutinize: that `Ok(None)` (not `Err`) is returned for Ctrl+C, keeping exit code 0.

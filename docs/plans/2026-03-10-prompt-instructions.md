# Prompt instructions override — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Let users override the default AI summary instructions via `ai.prompt_instructions` in config so they can get summaries in a different language or more concise. The structured block (period, commit count, commit list) stays fixed; only the instruction text is configurable.

**Architecture:** Add optional `prompt_instructions` to `AiConfig`; when set (non-empty after trim), pass it through to `build_prompt` as an override. `build_prompt` gets a third parameter `Option<&str>` and branches: default prompt vs custom instructions + structured block. Providers (CLI and API) receive the override at construction time and pass it into `build_prompt` when summarizing. Cache key in main uses the same prompt (including override) so cache stays correct.

**Tech Stack:** Rust, serde (TOML), existing `diddo` config and AI modules.

**Design doc:** `docs/plans/2026-03-10-prompt-instructions-design.md`

---

### Task 1: Config — add `prompt_instructions` and resolver

**Files:**
- Modify: `src/config.rs` (struct and impl)
- Test: `src/config.rs` (existing `#[cfg(test)]` module)

**Step 1: Add field and resolver**

In `src/config.rs`:

- Add to `AiConfig` (after `model`):
  ```rust
  pub prompt_instructions: Option<String>,
  ```
- In `impl AiConfig`, add:
  ```rust
  pub fn resolved_prompt_instructions(&self) -> Option<&str> {
      self.prompt_instructions
          .as_deref()
          .map(|s| s.trim())
          .filter(|s| !s.is_empty())
  }
  ```

**Step 2: Run tests**

Run: `cargo test -p diddo config::`
Expected: existing config tests still PASS (new field deserializes as `None` when missing).

**Step 3: Add test for parsing `prompt_instructions`**

In `src/config.rs` inside the test module, add:

```rust
#[test]
fn parses_prompt_instructions_from_toml() {
    let temp = temp_dir("prompt-instructions-parse");
    let config_path = temp.join("config.toml");

    fs::write(
        &config_path,
        r#"[ai]
prompt_instructions = " Summarize in German. One paragraph. "
"#,
    )
    .unwrap();

    let config = AppConfig::load(&config_path).unwrap();

    assert_eq!(
        config.ai.prompt_instructions.as_deref(),
        Some(" Summarize in German. One paragraph. ")
    );
    assert_eq!(
        config.ai.resolved_prompt_instructions(),
        Some("Summarize in German. One paragraph.")
    );

    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn prompt_instructions_empty_or_missing_returns_none() {
    let temp = temp_dir("prompt-instructions-empty");
    let missing = temp.join("config.toml");

    let config = AppConfig::load(&missing).unwrap();
    assert_eq!(config.ai.resolved_prompt_instructions(), None);

    let with_empty = temp.join("with_empty.toml");
    fs::write(&with_empty, r#"[ai]\nprompt_instructions = ""\n"#).unwrap();
    let config = AppConfig::load(&with_empty).unwrap();
    assert_eq!(config.ai.resolved_prompt_instructions(), None);

    let with_whitespace = temp.join("with_ws.toml");
    fs::write(&with_whitespace, r#"[ai]\nprompt_instructions = "  \n\t "\n"#).unwrap();
    let config = AppConfig::load(&with_whitespace).unwrap();
    assert_eq!(config.ai.resolved_prompt_instructions(), None);

    fs::remove_dir_all(temp).unwrap();
}
```

**Step 4: Run new tests**

Run: `cargo test -p diddo config::tests::parses_prompt_instructions config::tests::prompt_instructions_empty`
Expected: PASS.

**Step 5: Commit**

```bash
git add src/config.rs
git commit -m "feat(config): add ai.prompt_instructions and resolved_prompt_instructions()"
```

---

### Task 2: Prompt builder — add override parameter and custom branch

**Files:**
- Modify: `src/ai/mod.rs` (`build_prompt` and its tests)

**Step 1: Write failing test for custom instructions**

In `src/ai/mod.rs` in the test module, add:

```rust
#[test]
fn build_prompt_with_custom_instructions_uses_override_and_structured_block() {
    let commits = vec![sample_commit()];
    let prompt = build_prompt(&commits, "today", Some("Custom instructions here."));

    assert!(prompt.starts_with("Custom instructions here."));
    assert!(prompt.contains("Period: today"));
    assert!(prompt.contains("Commit count: 1"));
    assert!(prompt.contains("[diddo] feat: add AI summaries"));
    assert!(!prompt.contains("Return plain text only"));
}

#[test]
fn build_prompt_with_none_uses_default_instructions() {
    let commits = vec![sample_commit()];
    let prompt = build_prompt(&commits, "week", None);

    assert!(prompt.contains("You are summarizing git activity for week."));
    assert!(prompt.contains("Return plain text only. Keep it brief"));
    assert!(prompt.contains("Period: week"));
}
```

**Step 2: Run tests to see failure**

Run: `cargo test -p diddo ai::mod::tests::build_prompt_with`
Expected: FAIL (signature of `build_prompt` doesn’t take third param / no such tests yet if you only added one, so compile error or test failure).

**Step 3: Implement build_prompt with override**

- Change signature to:
  ```rust
  pub fn build_prompt(commits: &[Commit], period: &str, instructions_override: Option<&str>) -> String
  ```
- If `instructions_override` is `Some(s)`:
  - Start with `s.to_string()`.
  - Push `"\n\nPeriod: {period}\nCommit count: {}\n\nCommits:\n"` with `commits.len()`.
  - Append the same commit list as today (empty => "- No recorded commits.\n", else the numbered lines).
  - Return (no default closing).
- If `instructions_override` is `None`:
  - Keep the current implementation exactly (intro + period + count + commits + "Return plain text only...").
- Update existing test `prompt_includes_period_and_commit_details` to call `build_prompt(..., None)`.

**Step 4: Run tests**

Run: `cargo test -p diddo ai::mod::tests::`
Expected: All PASS.

**Step 5: Commit**

```bash
git add src/ai/mod.rs
git commit -m "feat(ai): build_prompt accepts optional instructions override"
```

---

### Task 3: CliProvider — pass instructions into prompt

**Files:**
- Modify: `src/ai/cli_provider.rs` (struct, constructor, summarize path, tests)
- Modify: `src/ai/mod.rs` (create_provider: pass override when constructing CliProvider)

**Step 1: Add field and constructor param**

In `src/ai/cli_provider.rs`:

- Add field to the struct that holds the runner (if any) or add a new struct field. Inspect current `CliProvider`: it has `tool: CliTool`. Add:
  ```rust
  prompt_instructions: Option<String>,
  ```
- Change `CliProvider::new(tool)` to `CliProvider::new(tool, prompt_instructions: Option<String>)` and set the field.
- In `summarize_with_runner`, replace:
  ```rust
  let prompt = build_prompt(commits, period);
  ```
  with:
  ```rust
  let prompt = build_prompt(commits, period, self.prompt_instructions.as_deref());
  ```

**Step 2: Wire config in create_provider**

In `src/ai/mod.rs`, change:
```rust
ProviderChoice::Cli(tool) => Ok(Box::new(CliProvider::new(tool)) as Box<dyn AiProvider>),
```
to:
```rust
ProviderChoice::Cli(tool) => Ok(Box::new(CliProvider::new(
    tool,
    config.resolved_prompt_instructions().map(String::from),
)) as Box<dyn AiProvider>),
```

**Step 3: Update CliProvider tests**

In `src/ai/cli_provider.rs`, every `CliProvider::new(CliTool::...)` becomes `CliProvider::new(CliTool::..., None)` (or `Some(...)` where you want to assert on custom prompt). Add one test that passes `Some("Custom.")` and asserts the prompt passed to the runner starts with "Custom."

**Step 4: Run tests**

Run: `cargo test -p diddo ai::cli_provider`
Expected: PASS.

**Step 5: Commit**

```bash
git add src/ai/cli_provider.rs src/ai/mod.rs
git commit -m "feat(ai): CliProvider uses config prompt_instructions"
```

---

### Task 4: ApiProvider — pass instructions into prompt

**Files:**
- Modify: `src/ai/api_provider.rs` (struct, from_config, new, summarize path, tests)
- Modify: `src/ai/mod.rs` if ApiProvider is constructed elsewhere

**Step 1: Add field and wire in from_config/new**

In `src/ai/api_provider.rs`:

- Add to struct: `prompt_instructions: Option<String>`.
- In `from_config`, after building api_key and model, add:
  ```rust
  let prompt_instructions = config.resolved_prompt_instructions().map(String::from);
  ```
  and pass it into `Self::new` (add parameter to `new`).
- In `new(kind, api_key, model, prompt_instructions: Option<String>)` set the field.
- In `summarize_with_client`, replace:
  ```rust
  let prompt = build_prompt(commits, period);
  ```
  with:
  ```rust
  let prompt = build_prompt(commits, period, self.prompt_instructions.as_deref());
  ```

**Step 2: Update ApiProvider tests**

Any test that constructs `ApiProvider::new(...)` must pass the new argument (e.g. `None`). Any test that checks the prompt passed to the request callback should optionally assert custom instructions when configured.

**Step 3: Run tests**

Run: `cargo test -p diddo ai::api_provider`
Expected: PASS.

**Step 4: Commit**

```bash
git add src/ai/api_provider.rs
git commit -m "feat(ai): ApiProvider uses config prompt_instructions"
```

---

### Task 5: main.rs — cache key and tests use same prompt

**Files:**
- Modify: `src/main.rs` (build_prompt call sites and any test that builds prompt)

**Step 1: Use config instructions for cache key prompt**

In `src/main.rs`, locate the block that does:
```rust
let prompt = ai::build_prompt(&commits, period);
let cache_key_opt = ai::primary_provider_identity(&config.ai)...
```
Change to:
```rust
let prompt = ai::build_prompt(
    &commits,
    period,
    config.ai.resolved_prompt_instructions(),
);
```
(Use `as_deref()` or equivalent so you pass `Option<&str>`; `config` is loaded right above so it’s in scope.)

**Step 2: Update test that builds prompt for cache**

In the test that calls `crate::ai::build_prompt(&commits, period)` for the cache key, change to:
```rust
let prompt = crate::ai::build_prompt(&commits, period, config.ai.resolved_prompt_instructions().as_deref());
```
so the cache key matches the prompt that would be used (that config has no prompt_instructions, so `None`).

**Step 3: Run tests**

Run: `cargo test -p diddo`
Expected: All PASS.

**Step 4: Commit**

```bash
git add src/main.rs
git commit -m "fix(main): use prompt_instructions in cache key and build_prompt calls"
```

---

### Task 6: README — document prompt_instructions

**Files:**
- Modify: `README.md` (Config File section)

**Step 1: Add example and note**

In the **Config File** section:

- In one of the example config blocks (e.g. the anthropic one), add an optional line:
  ```toml
  prompt_instructions = "Summarize in German. One short paragraph, plain text only."
  ```
- In the **Notes** list, add:
  - `ai.prompt_instructions` (optional): when set, replaces the default AI instructions (tone, language, length); period, commit count, and commit list are always appended. Use for a different language or more concise output. Empty or missing = default instructions.

**Step 2: Commit**

```bash
git add README.md
git commit -m "docs: document ai.prompt_instructions in README"
```

---

## Execution

Plan complete and saved to `docs/plans/2026-03-10-prompt-instructions.md`.

**Two execution options:**

1. **Subagent-driven (this session)** — Dispatch a fresh subagent per task, review between tasks, fast iteration.
2. **Parallel session (separate)** — Open a new session with executing-plans and run the plan in a worktree with checkpoints.

Which approach do you want?

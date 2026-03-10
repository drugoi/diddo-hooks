# Custom prompt instructions — Design

## Summary

Allow users to override the default AI summary instructions via config so they can get summaries in a different language, more concise, or with different tone. The structured block (period, commit count, commit list) stays fixed; only the instruction text is configurable. Config file only; no CLI flag.

## Decisions

- **Override scope:** Instructions only. We keep the structured block (period, commit count, commit list) and only the instruction part is configurable.
- **Where:** Config file only. Optional `ai.prompt_instructions` in `config.toml`.
- **Single key:** One optional string. When set (and non-empty after trim), it replaces the full default instruction block; when missing or empty, behavior is unchanged.
- **Empty = default:** Empty or whitespace-only value is treated as unset; we use the default prompt. No extra error or log.

## Config shape

Add to `AiConfig` in `src/config.rs`:

- `prompt_instructions: Option<String>` with `#[serde(default)]`.

Example in `config.toml`:

```toml
[ai]
provider = "anthropic"
model = "claude-3-7-sonnet-latest"
prompt_instructions = "Summarize in German. One short paragraph, plain text only."
```

When parsing, treat empty or whitespace-only string as `None` (store `None` so we don’t send a prompt with no instructions).

## Prompt assembly

- **Signature:** `build_prompt(commits, period, instructions_override: Option<&str>)`. Call sites pass `config.ai.prompt_instructions.as_deref()` (or a trimmed/non-empty helper).
- **Default (`instructions_override` is `None` or empty):** Unchanged: same intro, then "Period: …", "Commit count: …", "Commits:", commit list, then default closing ("Return plain text only. Keep it brief…").
- **Custom (`Some(s)`):** Prompt = custom instructions `s` + `"\n\nPeriod: {period}\nCommit count: {n}\n\nCommits:\n"` + same commit list as today. No extra closing sentence.
- **Shared:** Commit list format is unchanged (numbered lines: repo, message, hash, branch, time, files, +/−).

## Edge cases

- Empty/whitespace config value → use default prompt; no validation error.
- No length limit on `prompt_instructions`; we pass it through to the provider.
- Existing configs without the key → `None` via `#[serde(default)]`; no migration.

## Testing

- **Config:** Parse TOML with `prompt_instructions = "..."` and assert it’s present and trimmed; with empty or missing key assert `None`.
- **Prompt builder:** With `Some("Custom instructions.")` assert prompt starts with that text, contains period/count/commit block and list, and does not contain the default closing. With `None` assert output equals current default (or contains default intro and closing).
- **Integration:** One test that runs the summary path with config containing `prompt_instructions` and a mock provider that captures the prompt; assert captured prompt uses custom instructions.

## Docs

- **README:** In Config File section, add `prompt_instructions` to an example and one note: optional; when set replaces default instructions (tone, language, length); period/count/commit list always appended; use for different language or more concise output; empty or missing = default.

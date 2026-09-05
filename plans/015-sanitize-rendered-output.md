# Plan 015: Sanitize git-derived text at the render boundary; escape pipes in summary markdown tables

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 7a8b4ca..HEAD -- src/render.rs src/activity_report.rs`
> If the render functions below changed since this plan was written, compare
> excerpts before proceeding; on a mismatch, STOP.

## Status

- **Priority**: P2
- **Effort**: S
- **Risk**: LOW (output-only change; the only hazard is over-stripping, avoided by filtering control codes rather than allowlisting)
- **Depends on**: none
- **Category**: security + bug
- **Planned at**: commit `7a8b4ca`, 2026-09-05

## Why this matters

`diddo`/`diddo week`/`diddo standup` replay stored commit text to the terminal with no filtering. Commit messages are not all self-authored — merges, rebases, cherry-picks, and `git commit -C` import third-party message bodies, and they persist in SQLite to be replayed on every later summary. Raw ANSI/OSC escape sequences in that text enable output spoofing (fake status lines), cursor/scrollback manipulation, and on OSC-52-enabled terminals, clipboard writes. Additionally, the summary markdown table interpolates repository names without escaping `|`, breaking exported tables — while `activity_report.rs` already has the escaping helper and uses it; the two renderers have simply diverged. (Plan 008 makes newly recorded messages single-line subjects; rows recorded before it may still be multi-line — the sanitizer here also collapses those for display.)

## Current state

- `src/render.rs:368` (terminal listing inside `repos_to_*`):

```rust
            writeln!(writer, "{}  {}", commit.hash, commit.message)?;
```

- `src/render.rs:385` (markdown bullet):

```rust
            output.push_str(&format!("- `{}` {}\n", commit.hash, commit.message));
```

- `src/render.rs:281-286` (markdown activity table rows — `render_markdown_table`):

```rust
    for row in rows {
        output.push_str(&format!(
            "| {} | {} | {} |\n",
            row.repository, row.commit_count, row.percentage
        ));
    }
```

`row.repository` comes from `repo_table_rows` (~lines 483–493) and can be a repo *name* derived from the filesystem path.

- Repo names/branches are also written in headers: `src/render.rs:363` and `:422` region (`{repo_name} ({n} commits)` lines) — find every write of `repo_name`, `branch`, `message`, `profile_label` with `grep -n 'repo_name\|commit.message\|branch\|profile_label' src/render.rs`.

- The existing escaping helper — `src/activity_report.rs:333-335`:

```rust
fn escape_markdown_table_cell(text: &str) -> String {
    text.replace('|', "\\|")
}
```

with a test at ~line 611 (`grep -n 'escape_markdown' src/activity_report.rs`).

- `src/render.rs:1` is `#![allow(dead_code)]` module-wide — new pub helpers won't warn either way.
- JSON output (`render_json_by_profile`) uses serde — already correctly escaped; DO NOT touch it.
- Note: `render.rs` has a legacy pipeline (`SummaryData`, `render_terminal`, `render_markdown` at lines 11–99) that is dead/near-dead code per a separate audit finding. Apply sanitization in the **`_by_profile` functions** (lines 100–177 and the helpers they call, `repos_to_markdown`, `write_terminal`-family) — the live path. Touching the legacy trio is optional and low-value.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Render tests | `cargo test render::` | all pass |
| Full suite | `cargo test` | 0 failed |
| Lint/format | `cargo clippy -- -D warnings && cargo fmt -- --check` | exit 0 |

## Scope

**In scope**:
- `src/render.rs` — sanitizer helper + application at the live render call sites + tests
- `src/activity_report.rs` — replace its private `escape_markdown_table_cell` with the shared one (re-export or move), apply the sanitizer to its markdown/terminal message-adjacent fields ONLY if it renders commit text (it renders repo names — check `render_terminal`/`render_markdown` there; repo names flow through, so sanitize them too)

**Out of scope**:
- The JSON renderer (serde handles escaping).
- The AI prompt path (`ai::build_prompt`) — Plan 016 fences it.
- Truncating long messages — display truncation is a UX choice, not taken here.
- Deleting the legacy render pipeline.

## Git workflow

- Branch: `advisor/015-sanitize-rendered-output`
- Commit style: conventional commits, e.g. `fix: strip control sequences from rendered commit text and escape markdown cells`. No AI attribution.
- Do NOT push or open a PR unless instructed.

## Steps

### Step 1: The sanitizer

In `src/render.rs` add:

```rust
/// Strips C0 control characters (except '\t'), C1 controls (U+0080–U+009F),
/// and DEL from git-derived text, and collapses newlines to spaces, so stored
/// commit text cannot emit terminal escape sequences or break single-line layouts.
pub fn sanitize_for_display(text: &str) -> String {
    text.chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .filter(|c| {
            !c.is_control() && !('\u{80}'..='\u{9f}').contains(c) || *c == '\t'
        })
        .collect()
}
```

(Note the precedence: write it with explicit parens — the requirement is: keep `\t`; drop every other `char::is_control` and the C1 range; `\n`/`\r` already became spaces. `is_control` covers C0 and DEL; C1 chars are also `is_control` in Rust — verify with the tests below and simplify if redundant.)

Also add the shared cell escaper:

```rust
pub fn escape_markdown_table_cell(text: &str) -> String {
    text.replace('|', "\\|")
}
```

**Verify**: `cargo build` exits 0.

### Step 2: Apply at the live render boundaries

In the `_by_profile` pipeline of `src/render.rs`, wrap every interpolation of `commit.message`, `commit.hash` is safe (hex) — leave it, `repo_name`, `branch` (if rendered), and `profile_label` with `sanitize_for_display(...)`. For markdown table cells (`render_markdown_table` rows AND its header-adjacent repository fields), apply `escape_markdown_table_cell(&sanitize_for_display(...))`. Find all sites via the grep in Current state; expect roughly 6–10 interpolation points.

In `src/activity_report.rs`: delete its private `escape_markdown_table_cell`, `use crate::render::escape_markdown_table_cell;` instead (keep its existing test, pointing at the shared fn), and wrap repo-name interpolations in its terminal/markdown renderers with `sanitize_for_display`.

**Verify**: `cargo test` → 0 failed (existing render tests use plain fixtures; sanitization is identity on them).

### Step 3: Tests

In `src/render.rs`'s test module (model after neighboring render tests):

1. `sanitize_strips_ansi_escape_sequences` — input `"fix: ok\u{1b}[2K\u{1b}]52;c;evil\u{07}done"` → output contains `fix: ok` and `done`, contains no `\u{1b}` and no `\u{07}`.
2. `sanitize_collapses_newlines_keeps_tabs_and_unicode` — `"subject\nbody\tdetail — ünïcode"` → `"subject body\tdetail — ünïcode"`.
3. `terminal_render_sanitizes_commit_message` — drive the live terminal `_by_profile` renderer with a commit fixture whose message embeds `\u{1b}[31m`; assert the output has no escape byte. (Copy fixture construction from an existing `_by_profile` test.)
4. `markdown_table_escapes_pipe_in_repository_name` — a repo named `evil|repo` renders as `evil\|repo` in the table row.

**Verify**: `cargo test render::` → all pass including the 4 new tests. `cargo test` → 0 failed.

## Test plan

Step 3's four tests plus the relocated activity_report escaper test. Full gate `cargo test` 0 failed.

## Done criteria

- [ ] `cargo test` 0 failed incl. new tests
- [ ] `cargo clippy -- -D warnings`; `cargo fmt -- --check` exit 0
- [ ] `grep -c 'fn escape_markdown_table_cell' src/activity_report.rs` → 0 (moved to render.rs, single definition)
- [ ] `grep -c 'sanitize_for_display' src/render.rs` ≥ 5 (definition + call sites)
- [ ] Live terminal path: no raw `commit.message` interpolation remains in the `_by_profile` functions (`grep -n 'commit.message' src/render.rs` — every hit inside live code is wrapped)
- [ ] `git diff --name-only` ⊆ {src/render.rs, src/activity_report.rs, plans/README.md}

## STOP conditions

Stop and report back if:

- The `_by_profile` renderer structure differs materially from the line references (drift).
- Sanitization breaks an existing snapshot-style assertion in a way that reveals a *behavioral* dependency on control characters (unlikely; report it rather than weakening the sanitizer).
- You find yourself editing `render_json_by_profile` — out of scope.

## Maintenance notes

- Plan 016 reuses `sanitize_for_display` for prompt text — keep it `pub`.
- If message display truncation is added later, do it inside `sanitize_for_display`'s callers, not the sanitizer (single-responsibility).
- Reviewer should scrutinize: that `\t` survives (column alignment in terminal output) and non-ASCII text passes through untouched.

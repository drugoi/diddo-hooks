# Plan 012: Detect and invoke cursor-agent by its real binary name

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 7a8b4ca..HEAD -- src/ai/cli_provider.rs`
> Plan 011 may have added a timeout parameter to `run_cli_command` — that
> drift is fine. If `binary_name`/`display_name`/the CursorAgent match arm
> changed, compare excerpts; on mismatch, STOP.

## Status

- **Priority**: P2
- **Effort**: S
- **Risk**: LOW (users currently "detected" via the editor shim lose a provider that never worked)
- **Depends on**: none (compatible before or after 011; if 011 landed, keep its timeout plumbing intact)
- **Category**: bug
- **Planned at**: commit `7a8b4ca`, 2026-09-05

## Why this matters

The `CursorAgent` provider is detected and invoked as **`cursor`** — but `cursor` on PATH is the Cursor *editor's* launcher shim, not the agent CLI. Consequences: (a) anyone with the Cursor editor installed is falsely detected as having an AI provider; the fallback chain then wastes a try on `cursor agent <prompt> --no-interactive`, which is not a supported invocation of the editor shim; (b) a user who actually installed the real `cursor-agent` binary is never detected, and `ai.cli.prefer = "cursor-agent"` reports "preferred CLI tool cursor-agent is not installed". Every other name site already says `cursor-agent`: `from_name` accepts `"cursor-agent" | "cursor_agent"`, `display_name()` returns `"cursor-agent"`, the README and error text advertise `cursor-agent`.

## Current state

- `src/ai/cli_provider.rs:28-44`:

```rust
    pub fn binary_name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Opencode => "opencode",
            Self::CursorAgent => "cursor",        // <-- the bug
        }
    }

    pub fn display_name(self) -> &'static str {
        ...
            Self::CursorAgent => "cursor-agent",
        ...
    }
```

- `src/ai/cli_provider.rs:182-203` — `run_cli_command` match arm:

```rust
        CliTool::CursorAgent => {
            command.arg("agent");
            command.arg(prompt);
            command.arg("--no-interactive");
        }
```

(The `agent` subcommand exists on the *editor* shim's CLI surface; the standalone `cursor-agent` binary takes the prompt directly.)

- Detection: `detect_installed_tools` (lines 100–110) filters by `command_exists(tool.binary_name())`.
- Tests: 5 in the file, using an injected runner — none assert binary names.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Targeted tests | `cargo test cli_provider` | all pass |
| Full suite | `cargo test` | 0 failed |
| Lint/format | `cargo clippy -- -D warnings && cargo fmt -- --check` | exit 0 |
| Live check (only if installed) | `cursor-agent --help` | usage text listing a print/non-interactive flag |

## Suggested executor toolkit

- If network access is available, confirm the current non-interactive invocation in Cursor's CLI docs: https://cursor.com/docs/cli (the CLI is invoked as `cursor-agent`, with `-p`/`--print` for non-interactive output). If neither the binary nor docs are reachable, use the invocation specified in Step 2 and flag it for manual verification in your report — the *detection* fix is certain either way.

## Scope

**In scope**:
- `src/ai/cli_provider.rs` only (+ its tests)

**Out of scope**:
- README (already says `cursor-agent` everywhere).
- Adding new tools; touching other providers' argv.
- Timeout plumbing from Plan 011 (leave intact if present).

## Git workflow

- Branch: `advisor/012-fix-cursor-agent-invocation`
- Commit style: conventional commits, e.g. `fix: detect cursor-agent by its real binary name`. No AI attribution.
- Do NOT push or open a PR unless instructed.

## Steps

### Step 1: Fix the binary name

Change the `binary_name` arm: `Self::CursorAgent => "cursor-agent",`.

**Verify**: `cargo build` exits 0.

### Step 2: Fix the invocation

Replace the `CursorAgent` arm in `run_cli_command`:

```rust
        CliTool::CursorAgent => {
            command.arg("-p");
            command.arg(prompt);
        }
```

(`cursor-agent -p "<prompt>"` runs one non-interactive turn and prints the result — the same shape as `claude -p`. Drop the `agent` subcommand and `--no-interactive`.) If the live check or docs contradict this flag, use what they say and note it.

**Verify**: `cargo build` exits 0.

### Step 3: Lock name consistency with a test

Add to the file's test module:

```rust
    #[test]
    fn binary_names_match_display_names() {
        for tool in [CliTool::Claude, CliTool::Codex, CliTool::Opencode, CliTool::CursorAgent] {
            assert_eq!(tool.binary_name(), tool.display_name());
        }
    }
```

This is now true for all four and prevents the class of bug recurring.

**Verify**: `cargo test cli_provider` → all pass.

### Step 4: Live sanity check (best-effort)

If `cursor-agent` is installed on this machine (`command -v cursor-agent`), run `cursor-agent -p "Reply with the single word OK"` and confirm output. If not installed, state in your report that the invocation follows the documented `-p` form and needs one manual verification by someone with the tool.

**Verify**: output contains `OK`, or the report notes the manual-verify flag.

## Test plan

Step 3's consistency test; existing runner-injected tests unaffected. Full gate `cargo test` 0 failed.

## Done criteria

- [ ] `grep -n '"cursor"' src/ai/cli_provider.rs` → no matches (only `"cursor-agent"` remains)
- [ ] `grep -c '"agent"' src/ai/cli_provider.rs` → 0
- [ ] `cargo test` 0 failed incl. the new consistency test
- [ ] `cargo clippy -- -D warnings`; `cargo fmt -- --check` exit 0
- [ ] `git diff --name-only` ⊆ {src/ai/cli_provider.rs, plans/README.md}

## STOP conditions

Stop and report back if:

- `binary_name` already returns `"cursor-agent"` (fixed independently — mark the plan REJECTED in the index with that note).
- Docs/live check reveal `cursor-agent` requires additional mandatory flags for non-interactive use that conflict with the prompt-as-single-arg pattern — report the exact required invocation rather than guessing.

## Maintenance notes

- Cache-key note: `primary_provider_identity` keys CLI providers by `display_name()`, which was already `"cursor-agent"` — no cache invalidation results from this fix.
- When Plan 016 adds restrictive flags per CLI, the cursor-agent arm is where its read-only/tool-restriction flag would go.
- Reviewer should scrutinize: nothing else — this is deliberately a three-line fix plus a guard test.

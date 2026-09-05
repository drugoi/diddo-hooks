# Plan 006: Honor output flags when stdout is piped, reject unknown flags

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 7a8b4ca..HEAD -- src/main.rs README.md`
> If `parse_cli` or `main` in src/main.rs changed since this plan was
> written, compare the excerpts below before proceeding; on a mismatch, STOP.

## Status

- **Priority**: P1
- **Effort**: S
- **Risk**: LOW (the only behavior that changes is a path that is currently wrong)
- **Depends on**: none
- **Category**: bug
- **Planned at**: commit `7a8b4ca`, 2026-09-05

## Why this matters

`diddo --json > out.json` exits 0 and writes **human-formatted terminal text**. The flag is parsed, then thrown away: when every argument starts with `-`, `parse_cli` returns default `SummaryArgs` without ever handing the args to clap. In a terminal this is the documented behavior (bare-ish invocations open interactive mode and ignore flags), but when stdin/stdout are not TTYs the interactive branch is skipped and the flags are silently dropped — scripts and CI get wrong-format data with no error. The same shortcut means a typo like `diddo --tabel` is never rejected: in a terminal it silently opens the menu; piped, it silently prints today's default summary.

The fix is one branch change: the flags-only path in `parse_cli` should parse through clap (`TodayCli`) and keep the parsed flags. The documented TTY behavior is untouched because `main()` routes TTY bare invocations to interactive mode *before* `parse_cli` is ever called.

## Current state

- `src/main.rs:186-233` — `parse_cli`. The relevant branch (lines 222–227):

```rust
    if only_option_args {
        return Ok(ParsedCli {
            command: None,
            summary: SummaryArgs::default(),
        });
    }
```

`only_option_args` (lines 203–206) is true when every arg after the binary name starts with `-`. Earlier branches: explicit `-h/--help/-V/--version` → `HelpCli` (lines 208–213); no second arg → `TodayCli::try_parse_from` (lines 215–220, already honors flags for the zero-arg case).

- `src/main.rs:256-284` — `main()`. `is_bare_invocation` (lines 258–263) is true when there are no args or all args are `-`-prefixed (excluding help/version forms). TTY + bare → `interactive::run(...)` and `parse_cli` is only reached via `parse_interactive_selection` (line 249), whose input always begins with a subcommand word (menu keys like `today`, `range --from ...`), so `only_option_args` is false on that path. Non-TTY (piped) or non-bare → `parse_cli(raw_args)` at line 282. **Therefore the `only_option_args` branch of `parse_cli` is reachable only for piped/redirected flag-only invocations** — exactly the case that should honor flags.

- `TodayCli` (lines 44–53) is `#[derive(Parser)]` with a flattened `SummaryArgs` (lines 55–81: `--md`, `--raw`, `--json`, `--table`, `--no-cache`, with an ArgGroup making md/raw/json/table mutually exclusive).

- Tests locking the old behavior live in the `mod tests` of `src/main.rs` (module starts ~line 1015). Find them with: `grep -n 'only_option\|option_args\|ignores' src/main.rs` — expect tests around lines 1161–1210 asserting that flag-only argv yields `SummaryArgs::default()`.

- `README.md:141` documents: "Any `--` flags without a subcommand (e.g. `diddo --table`, `diddo --md`) also launch interactive mode; the flags are ignored." — still true for TTYs; the piped case needs a clarifying sentence. Same for the "Current CLI behavior" bullet at README.md:202.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Targeted tests | `cargo test parse_cli` and `cargo test tests::` | all pass |
| Full suite | `cargo test` | 0 failed |
| Lint/format | `cargo clippy -- -D warnings && cargo fmt -- --check` | exit 0 |
| Behavior check | `cargo run --quiet -- --json < /dev/null \| head -c1` | `{` (JSON) or the empty-period JSON — NOT a blank/terminal line |
| Typo check | `cargo run --quiet -- --tabel < /dev/null; echo "exit=$?"` | clap error mentioning `--tabel`, exit=2 |

(`< /dev/null` makes stdin non-TTY so the interactive branch is skipped even in your terminal; stdout piping through `head` covers stdout.)

## Scope

**In scope**:
- `src/main.rs` — `parse_cli` only, plus its tests
- `README.md` — the two flag-behavior sentences

**Out of scope**:
- `is_bare_invocation` / `main()` routing — do not touch; TTY behavior is documented and correct.
- The three near-identical clap structs (`HelpCli`/`CommandCli`/`TodayCli`) — consolidation is a separate tech-debt finding; leave them.
- `interactive.rs`, `summary_request_from_cli`.

## Git workflow

- Branch: `advisor/006-honor-flags-when-piped`
- Commit style: conventional commits, e.g. `fix: honor output flags in non-interactive invocations and reject unknown flags`. No AI attribution.
- Do NOT push or open a PR unless instructed.

## Steps

### Step 1: Route the flags-only branch through clap

Replace lines 222–227 of `src/main.rs` with:

```rust
    if only_option_args {
        return TodayCli::try_parse_from(args).map(|cli| ParsedCli {
            command: None,
            summary: cli.summary,
        });
    }
```

This both honors the parsed flags and makes clap reject unknown ones (`Err(clap::Error)` → `error.exit()` at the call site prints the standard clap diagnostic and exits 2).

**Verify**: `cargo build` exits 0.

### Step 2: Update the tests that locked the old behavior

Run `cargo test` and fix ONLY the failures in tests that asserted flag-only argv produces `SummaryArgs::default()`. Update them to assert the flags are now honored, e.g. argv `["diddo", "--json"]` → `command: None`, `summary.json == true`. Add two new tests in the same module, modeled on the neighboring `parse_cli` tests:

1. `parse_cli_honors_output_flags_without_subcommand` — `["diddo", "--md", "--no-cache"]` → `summary.md && summary.no_cache`.
2. `parse_cli_rejects_unknown_flag_without_subcommand` — `["diddo", "--tabel"]` → `Err`.

Also confirm the mutual-exclusion group now applies: `["diddo", "--md", "--json"]` → `Err` (add as a third assertion if no existing test covers it).

**Verify**: `cargo test` → 0 failed.

### Step 3: Behavior checks and README

Run the two behavior-check commands from the table; confirm expected output. Then update `README.md`:

- Line ~141: append a sentence such as: "When output is piped or redirected (not a terminal), flags without a subcommand are honored instead: `diddo --json > out.json` emits JSON."
- The "Current CLI behavior" bullet (~line 202) and the "Output flags must be used with a subcommand" line (~line 174): adjust to say flags without a subcommand are ignored *in interactive (terminal) mode* and honored when piped. While editing line 174, also fix a pre-existing doc bug the audit found: the subcommand list omits `month` and `range`, which do accept output flags — add them.

**Verify**: `grep -n 'piped' README.md` shows the new sentence; behavior commands pass.

## Test plan

Step 2's updated + 3 new tests, plus the two runtime checks. Full-suite gate `cargo test`.

## Done criteria

- [ ] `cargo run --quiet -- --json < /dev/null` emits JSON (starts with `{` or `[`)
- [ ] `cargo run --quiet -- --tabel < /dev/null` exits 2 with a clap error
- [ ] `diddo` in a TTY with `--table` still opens interactive mode (cannot be tested non-interactively — verify by code inspection that `main()` lines 256–284 are untouched; state this in the report)
- [ ] `cargo test` 0 failed; `cargo clippy -- -D warnings`; `cargo fmt -- --check` exit 0
- [ ] `git diff --name-only` ⊆ {src/main.rs, README.md, plans/README.md}

## STOP conditions

Stop and report back if:

- `parse_cli` no longer matches the excerpt (drift — especially if someone consolidated the clap structs).
- More than ~4 existing tests fail after Step 1 — the blast radius should be small; a larger failure set means an assumption is wrong.
- `parse_interactive_selection` turns out to feed flags-only strings into `parse_cli` (check `MENU_ITEMS` keys in `src/interactive.rs` — every key must start with a subcommand word). If any key is flags-only, STOP: the interactive path would change behavior.

## Maintenance notes

- If interactive mode ever gains a "launch with flags" feature, revisit `is_bare_invocation` and this branch together — they encode the same rule in two places (known tech-debt finding).
- Reviewer should scrutinize: exit code 2 (clap convention) vs the repo's exit 1 for runtime errors — this plan intentionally keeps clap's convention for parse errors, matching existing `error.exit()` behavior for subcommand parse failures.

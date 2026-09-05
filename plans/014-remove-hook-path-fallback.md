# Plan 014: Remove the bare-`diddo` PATH fallback from generated hook scripts

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 7a8b4ca..HEAD -- src/init.rs`
> Plan 003 is expected to have modified the script builders (local-hook
> forwarding). Read the CURRENT `build_post_commit_script` and
> `build_post_commit_script_with_previous` before editing; the fallback lines
> this plan removes may have shifted. If the `else diddo hook` pattern is
> absent entirely, treat as STOP (fixed independently).

## Status

- **Priority**: P2
- **Effort**: S
- **Risk**: LOW
- **Depends on**: plans/003-forward-repo-local-hooks.md (same functions; land 003 first)
- **Category**: security
- **Planned at**: commit `7a8b4ca`, 2026-09-05

## Why this matters

The generated `post-commit` scripts embed the absolute path of the diddo binary, but fall back to a bare `diddo hook` — a `$PATH` lookup at commit time — whenever the recorded path is missing or non-executable. That state is exactly what exists after an uninstall, a Homebrew relink, or an interrupted `diddo update`. Hooks run in the developer's shell environment with their full privileges; if `$PATH` ever contains a directory a project can write to (a project-local `node_modules/.bin` via direnv, a tool-manager shim dir), the hook executes whatever `diddo` resolves to, on every commit. The path-quoting in the scripts is otherwise sound (`shell_single_quote`); this fallback is the one injection-shaped hole. The robustness goal it served ("never break the commit") is preserved by replacing it with an explicit skip-with-message.

## Current state

(At `7a8b4ca`; plan 003 may have moved these — read the live file first.)

- `src/init.rs:159-172` — `build_post_commit_script`:

```rust
    match resolve_diddo_executable() {
        Ok(Some(diddo_path)) => {
            script.push_str(&format!(
                "diddo_path={}\nif [ -x \"$diddo_path\" ]; then\n  \"$diddo_path\" hook || diddo_status=$?\nelse\n  diddo hook || diddo_status=$?\nfi\n",
                shell_single_quote(&path_for_script(&diddo_path))
            ));
        }
        _ => script.push_str("diddo hook || diddo_status=$?\n"),
    }
```

- `src/init.rs:506-519` — `build_post_commit_script_with_previous` contains the identical pattern (used for local-hooks-dir installs, e.g. Husky repos).
- `resolve_diddo_executable` returns `Ok(Some(path))` from `std::env::current_exe()` (canonicalized) — find it with `grep -n 'fn resolve_diddo_executable' src/init.rs`.
- Script conventions: `#!/bin/sh` + `set -u`, `# diddo-managed` marker, statuses accumulated in `diddo_status` and re-raised at the end. Tests assert on script contents as strings in the inline test module.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Targeted tests | `cargo test init::` | all pass |
| Full suite | `cargo test` | 0 failed |
| Lint/format | `cargo clippy -- -D warnings && cargo fmt -- --check` | exit 0 |

## Scope

**In scope**:
- `src/init.rs` — the two `diddo`-invocation emitters + tests

**Out of scope**:
- The previous-hook and local-hook forwarding blocks (plan 003's work).
- `resolve_diddo_executable` itself.
- Prepending a fixed `PATH=` inside scripts — considered and rejected: it would break users' git installed outside `/usr/bin:/bin` (Homebrew git lives in `/opt/homebrew/bin`); the fallback removal alone closes the hole.

## Git workflow

- Branch: `advisor/014-remove-hook-path-fallback`
- Commit style: conventional commits, e.g. `fix: never resolve diddo via PATH from generated hooks`. No AI attribution.
- Do NOT push or open a PR unless instructed.

## Steps

### Step 1: Replace the fallback in both builders

In both `build_post_commit_script` and `build_post_commit_script_with_previous`, change the emitted fragment to:

```sh
diddo_path='<quoted absolute path>'
if [ -x "$diddo_path" ]; then
  "$diddo_path" hook || diddo_status=$?
else
  echo "diddo: binary not found at $diddo_path; skipping commit recording (reinstall diddo or rerun 'diddo init')" >&2
fi
```

(Rust format string: keep using `shell_single_quote(&path_for_script(&diddo_path))`; only the `else` branch changes.) For the `Ok(None)`/`Err` arm (`_ =>`), where no path could be resolved at install time, emit *only* the skip message:

```rust
        _ => script.push_str(
            "echo \"diddo: could not resolve diddo binary at install time; skipping commit recording (rerun 'diddo init')\" >&2\n",
        ),
```

`diddo_status` stays 0 in both skip paths, so the user's commit is never broken — the property the fallback existed for.

**Verify**: `cargo build` exits 0.

### Step 2: Update the script-content tests, add the guard test

Fix any existing tests asserting the old `else\n  diddo hook` fragment. Add:

```rust
    #[test]
    fn generated_scripts_never_invoke_diddo_via_path_lookup() {
        for script in [
            build_post_commit_script(None /*, plan-003 params as applicable */),
            build_post_commit_script_with_previous("/some/hooks", "post-commit.diddo-prev"),
        ] {
            for line in script.lines() {
                let trimmed = line.trim_start();
                assert!(
                    !trimmed.starts_with("diddo hook") && !trimmed.starts_with("diddo "),
                    "unqualified diddo invocation in generated script: {line}"
                );
            }
        }
    }
```

(Adjust the constructor calls to whatever signatures plan 003 left; the assertion is the point: no line may invoke `diddo` unqualified — the quoted `"$diddo_path" hook` form and echo messages pass, a bare `diddo hook` fails.)

**Verify**: `cargo test init::` → all pass.

### Step 3: Confirm the skip path is non-fatal

Sandbox check (no global config touched):

```sh
tmp=$(mktemp -d) && cd "$tmp" && git init -q
printf '#!/bin/sh\nset -u\ndiddo_path=/nonexistent/diddo\nif [ -x "$diddo_path" ]; then\n  "$diddo_path" hook\nelse\n  echo "diddo: binary not found at $diddo_path; skipping commit recording" >&2\nfi\n' > .git/hooks/post-commit
chmod +x .git/hooks/post-commit
git commit -q --allow-empty -m test && echo "commit-ok"
```

Expected: the skip message on stderr AND `commit-ok` (exit 0). Clean up `$tmp`.

**Verify**: both outputs present.

## Test plan

Step 2's guard test + updated content assertions; Step 3's sandbox. Full gate `cargo test` 0 failed.

## Done criteria

- [ ] `grep -n 'else\\\\n  diddo hook\|_ => script.push_str("diddo hook' src/init.rs` → no matches; more simply: `grep -c '  diddo hook' src/init.rs` → 0
- [ ] Guard test present and passing
- [ ] `cargo test` 0 failed; `cargo clippy -- -D warnings`; `cargo fmt -- --check` exit 0
- [ ] Step 3 sandbox: message + successful commit
- [ ] `git diff --name-only` ⊆ {src/init.rs, plans/README.md}

## STOP conditions

Stop and report back if:

- The fallback pattern is already gone (mark REJECTED/DONE-independently in the index).
- Plan 003's builder signatures make the test constructors ambiguous — read 003's diff and match it; if 003 has NOT landed, STOP and execute it first (dependency order).

## Maintenance notes

- After this lands, a moved/deleted binary means commits are *not recorded* until `diddo init` reruns — the skip message tells users. `diddo metadata`'s hook-status hints are where discoverability could improve later.
- `diddo update` replaces the binary at the same path, so the recorded path stays valid across updates; Homebrew relinks keep `/opt/homebrew/bin/diddo` stable. The message covers the residual cases.
- Reviewer should scrutinize: the `_ =>` arm — it must not leave a script that exits non-zero.

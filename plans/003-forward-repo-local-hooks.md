# Plan 003: Keep repo-local .git/hooks running after `diddo init`

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 7a8b4ca..HEAD -- src/init.rs README.md`
> If `src/init.rs` changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: M
- **Risk**: MED (behavior change: dormant repo-local hooks start firing again — which is the correct behavior)
- **Depends on**: none (but must land BEFORE plan 014, which edits the same script builders)
- **Category**: bug
- **Planned at**: commit `7a8b4ca`, 2026-09-05

## Why this matters

`diddo init` sets the **global** `core.hooksPath` to diddo's managed hooks directory. Git then looks *only* in that directory for hooks — `$GIT_DIR/hooks` in every repository is ignored entirely. The managed directory contains a `post-commit` script and, only when the user previously had a *global* `core.hooksPath`, forwarding wrappers to that previous global path. There is no fallback to the per-repository `.git/hooks` directory anywhere in the generated scripts.

Consequence: after `diddo init`, every hook a user has in `.git/hooks` in every repo silently stops firing. The most common casualty is `git lfs install`, which writes `pre-push`, `post-checkout`, `post-commit`, and `post-merge` into `.git/hooks` — LFS uploads silently stop. The README claim "Preserves and forwards any previously configured global hooks so existing hooks keep running" is true only for the previous-*global*-path case.

The fix: every hook in the managed directory (all 22 names, always created) must also invoke the repo-local `$GIT_DIR/hooks/<name>` if it exists and is executable.

## Current state

- `src/init.rs` — hook installation/uninstall. Key pieces at commit `7a8b4ca`:

Constants (lines 30–57): `POST_COMMIT_FILE = "post-commit"`, `DIDDO_MANAGED_MARKER = "# diddo-managed"`, `STATE_FILE = "diddo-managed-state"`, and `HOOK_NAMES: &[&str]` listing all 22 git hook names including `"post-commit"`.

`install_with` (lines 219–251) — the relevant part:

```rust
    if let Some(previous_hooks_dir) = previous_hooks_dir.as_ref() {
        create_forwarding_hooks(&previous_hooks_dir.raw, &paths.hooks_dir)?;
    }

    let generated_post_commit = paths.hooks_dir.join(POST_COMMIT_FILE);
    fs::write(
        &generated_post_commit,
        build_post_commit_script(previous_hooks_dir.as_ref().map(|state| state.raw.as_str())),
    )?;
```

`create_forwarding_hooks` (lines 352–367) writes a wrapper for every `HOOK_NAMES` entry except `post-commit`, but is **only called when `previous_hooks_dir` is `Some`**.

`build_forwarding_hook_script` (lines 192–197):

```rust
fn build_forwarding_hook_script(previous_hooks_path: &str, hook_name: &str) -> String {
    format!(
        "#!/bin/sh\nset -eu\n\n{}if [ -x \"$previous_hook_path\" ]; then\n  \"$previous_hook_path\" \"$@\"\nfi\n",
        build_previous_hook_path_resolution(previous_hooks_path, hook_name)
    )
}
```

`build_post_commit_script` (lines 159–190) runs `diddo hook`, then (only if a previous global path was recorded) the previous `post-commit`, then exits with the first non-zero status.

`build_previous_hook_path_resolution` (lines 210–217) resolves only the recorded previous path (absolute/Windows/`~`/relative forms). There is **no** `$GIT_DIR/hooks` reference anywhere in the file (verify: `grep -c 'git-dir\|GIT_DIR' src/init.rs` → 0).

- Repo conventions: script builders are pure functions returning `String`, tested by string assertions in the inline `#[cfg(test)] mod tests` (starting ~line 700). Install/uninstall logic is tested through the DI seams `install_with`/`uninstall_with` with closures and `tempfile`-free temp dirs (the tests build their own temp dirs via `std::env::temp_dir()` — follow whatever pattern the existing tests at `src/init.rs:844-1054` use).
- One subtlety you must preserve: `shell_single_quote` (line ~637) is used for every path interpolated into scripts. Any new interpolated path must go through it. Runtime-computed paths (from `git rev-parse`) are shell variables, not interpolations — quote them as `"$var"`.

**Why `git rev-parse --git-dir` and not `--git-path hooks`**: `git rev-parse --git-path hooks` *honors `core.hooksPath`* and would return the managed directory itself — infinite recursion. `"$(git rev-parse --git-dir)/hooks"` is the raw per-repo hooks directory, which is exactly what git would have used without `core.hooksPath`.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Tests | `cargo test init::` | all init tests pass |
| Full suite | `cargo test` | 0 failed |
| Lint | `cargo clippy -- -D warnings` | exit 0 |
| Format | `cargo fmt` then `cargo fmt -- --check` | exit 0 |

## Scope

**In scope**:
- `src/init.rs` (script builders, `install_with`, `create_forwarding_hooks`, tests)
- `README.md` (the "What `diddo init` does" bullet list, ~lines 77–82)

**Out of scope**:
- The bare-`diddo` PATH fallback inside `build_post_commit_script` — Plan 014 removes it; leave it exactly as-is here.
- `install_local_hook` / the Husky path (lines 465–504) — local-hooks-dir repos are unaffected by global hooksPath.
- `uninstall_with`, `hooks_status`, `src/main.rs`.

## Git workflow

- Branch: `advisor/003-forward-repo-local-hooks`
- Commit style: conventional commits, e.g. `fix: forward repo-local .git/hooks from managed global hooks`. No AI attribution.
- Do NOT push or open a PR unless instructed.

## Steps

### Step 1: Add a repo-local fallback block to the forwarding script builder

Create a helper that emits the shell fragment (note `2>/dev/null || true` — outside a git repo, `rev-parse` must not kill the hook, and `set -e` is active in forwarding scripts):

```rust
fn build_local_hook_invocation(hook_name: &str, status_var: Option<&str>) -> String {
    // status_var: Some => record exit code (post-commit style), None => propagate via set -e
    ...
}
```

Emitted shell shape (for the `set -eu` forwarding scripts, `status_var = None`):

```sh
local_git_dir="$(git rev-parse --git-dir 2>/dev/null || true)"
if [ -n "$local_git_dir" ]; then
  local_hook_path="$local_git_dir/hooks/<hook_name>"
  if [ -x "$local_hook_path" ]; then
    "$local_hook_path" "$@"
  fi
fi
```

`<hook_name>` is interpolated at build time via `shell_single_quote` into a `local_hook_name=...` variable assignment (mirror how `build_previous_hook_path_resolution` handles `previous_hook_name`), then use `"$local_git_dir/hooks/$local_hook_name"`.

**Verify**: `cargo build` exits 0 (helper compiles; not yet wired).

### Step 2: Wire the fallback into both script builders

1. `build_forwarding_hook_script(previous_hooks_path: Option<&str>, hook_name: &str)` — change the first parameter to `Option<&str>`. Script layout: shebang + `set -eu`, then (if `Some`) the existing previous-hook block, then the new local-hook block from Step 1. Order matters: previous-global first (that is what ran before diddo), then repo-local (which git itself would have run last is actually the *only* thing git ran before — order previous-global then local is fine; document the order in a comment in the script template).
2. `build_post_commit_script(previous_hooks_path: Option<&str>)` — after the existing previous-hook block and before the final `diddo_status` exit check, add the local-hook block in status-recording form (`set -u` only, no `-e`, matching the existing style):

```sh
local_status=0
local_git_dir="$(git rev-parse --git-dir 2>/dev/null || true)"
if [ -n "$local_git_dir" ]; then
  local_hook_path="$local_git_dir/hooks/post-commit"
  if [ -x "$local_hook_path" ]; then
    "$local_hook_path" "$@" || local_status=$?
  fi
fi
if [ "$local_status" -ne 0 ]; then
  exit "$local_status"
fi
```

3. **Recursion guard**: none needed beyond the design — the managed dir never lives inside a repo's `$GIT_DIR`, and `--git-dir` ignores `core.hooksPath`. But add a build-time guard anyway: the local block must NOT be emitted for scripts written *into a repo's own hooks dir* by `install_local_hook` (out of scope here, but `build_post_commit_script(None)` is shared with it — see next point).

4. `install_local_hook` (line 497) calls `build_post_commit_script(None)` for the Husky case. A local-hooks-dir script invoking `$GIT_DIR/hooks/post-commit` would be wrong there only if `core.hooksPath` (local) differs from `$GIT_DIR/hooks` — which is exactly the Husky case, and `$GIT_DIR/hooks/post-commit` might contain a stale hook. To keep `install_local_hook` behavior unchanged, add a boolean parameter or a separate builder: `build_post_commit_script(previous, include_local_fallback: bool)` — pass `true` from `install_with`, `false` from `install_local_hook`. Same for `build_post_commit_script_with_previous` (leave it without the fallback).

**Verify**: `cargo build` exits 0.

### Step 3: Always create forwarding hooks

In `install_with`, replace:

```rust
    if let Some(previous_hooks_dir) = previous_hooks_dir.as_ref() {
        create_forwarding_hooks(&previous_hooks_dir.raw, &paths.hooks_dir)?;
    }
```

with an unconditional call, passing the optional previous path through:

```rust
    create_forwarding_hooks(
        previous_hooks_dir.as_ref().map(|state| state.raw.as_str()),
        &paths.hooks_dir,
    )?;
```

and update `create_forwarding_hooks` to accept `Option<&str>`.

**Verify**: `cargo test init::` — expect failures ONLY in tests asserting the managed dir contains just `post-commit` + state file after a fresh install (e.g. the test around `src/init.rs:844-874`). Update those tests in Step 4; any other failure is a STOP condition.

### Step 4: Update and extend the tests

Update existing assertions: a fresh install (no previous global path) now produces all 22 `HOOK_NAMES` files + the state file in the managed dir.

New tests (in the existing `#[cfg(test)] mod tests`, following the existing string-assertion style):

1. `forwarding_script_without_previous_contains_local_fallback` — `build_forwarding_hook_script(None, "pre-push")` contains `git rev-parse --git-dir` and `hooks/$local_hook_name` (or the exact emitted line), and does NOT contain `previous_hook_path`.
2. `forwarding_script_with_previous_runs_previous_then_local` — with `Some("/prev/hooks")`: previous block appears before local block.
3. `post_commit_script_contains_local_fallback_for_global_install` — `build_post_commit_script(None, true)` contains the local block and the `local_status` exit propagation.
4. `local_install_post_commit_has_no_local_fallback` — the script written by `install_local_hook` (or `build_post_commit_script(None, false)`) does NOT contain `git rev-parse --git-dir`.
5. `fresh_install_creates_all_hook_wrappers` — via `install_with` with mocked closures: managed dir contains every name in `HOOK_NAMES`.

**Verify**: `cargo test init::` → all pass. `cargo test` → 0 failed.

### Step 5: Manual end-to-end sanity check (in a throwaway environment)

Do NOT run `diddo init` against the real global git config. Instead:

```sh
tmp=$(mktemp -d) && cd "$tmp" && git init -q repo && cd repo
printf '#!/bin/sh\necho LOCAL-HOOK-RAN >> "$PWD/hook.log"\n' > .git/hooks/post-commit
chmod +x .git/hooks/post-commit
mkdir "$tmp/managed"
# simulate: copy the generated post-commit script (print it from a tiny test or use the one your tests assert) into "$tmp/managed"
git config --local core.hooksPath "$tmp/managed"
git commit -q --allow-empty -m test
cat hook.log
```

Expected: `hook.log` contains `LOCAL-HOOK-RAN` (the managed script forwarded to `.git/hooks/post-commit`). The `diddo hook` line inside the script will fail harmlessly if `diddo` isn't on PATH in the sandbox — that's fine; you're verifying the local-forward line. Clean up `$tmp` afterwards.

**Verify**: `LOCAL-HOOK-RAN` appears; the commit itself succeeded (exit 0).

### Step 6: Update the README

In the "What `diddo init` does" list (README.md ~lines 77–82), change the preserve/forward bullet to state both behaviors, e.g.:

- Preserves and forwards any previously configured global hooks
- Forwards to each repository's own `.git/hooks` (so repo-local hooks like git-lfs keep running)

**Verify**: `grep -n '.git/hooks' README.md` shows the new bullet.

## Test plan

Covered in Step 4 (5 new tests + updated assertions) and the Step 5 sandbox check. Model new tests after the existing script-content tests in `src/init.rs`'s test module.

## Done criteria

- [ ] `cargo test` → 0 failed, including the 5 new tests
- [ ] `cargo clippy -- -D warnings` and `cargo fmt -- --check` exit 0
- [ ] `grep -c 'rev-parse --git-dir' src/init.rs` ≥ 2 (both builders emit it)
- [ ] Step 5 sandbox shows `LOCAL-HOOK-RAN`
- [ ] `git diff --name-only` ⊆ {`src/init.rs`, `README.md`, `plans/README.md`}
- [ ] README bullet updated

## STOP conditions

Stop and report back if:

- `install_with`/`create_forwarding_hooks`/the script builders no longer match the excerpts (drift).
- Test failures in Step 3 outside the fresh-install-contents assertions.
- You find an existing mechanism that already forwards to `$GIT_DIR/hooks` (would mean this plan misread the code — it should not exist at `7a8b4ca`).
- The Step 5 sandbox does not run the local hook after two debugging attempts.

## Maintenance notes

- Plan 014 rewrites the `diddo`-invocation lines in the same builders — run it after this lands.
- Behavior change for release notes: repo hooks that were silently dead since `diddo init` will start running again; a user with a broken `.git/hooks/pre-push` will now see it fire. Worth a CHANGELOG line.
- If a future feature adds new hook names to `HOOK_NAMES`, the forwarding + local-fallback structure picks them up automatically — reviewer should confirm no hook name is special-cased except `post-commit`.
- Deferred: `uninstall` still deletes the managed dir wholesale; unaffected by this plan.

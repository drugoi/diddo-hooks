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
- **Risk**: LOW-MED (behavior change: repo-local hooks that `diddo init` silently killed start running again — restoring what git did before diddo)
- **Depends on**: none (but must land BEFORE plan 014, which edits the same script builders)
- **Category**: bug
- **Planned at**: commit `7a8b4ca`, 2026-09-05
- **Revised**: 2026-09-05 at commit `b4aff1d` — forwarding is now EXCLUSIVE (mirror git's own
  `core.hooksPath` semantics) rather than additive. The original plan ran the previous-global
  hook *and* the repo-local hook; that would resurrect `.git/hooks` entries a user's own global
  `core.hooksPath` had deliberately bypassed. Decided by the maintainer. Test list and done
  criteria updated to lock the exclusivity in.

## Why this matters

`diddo init` sets the **global** `core.hooksPath` to diddo's managed hooks directory. Git then looks *only* in that directory for hooks — `$GIT_DIR/hooks` in every repository is ignored entirely. The managed directory contains a `post-commit` script and, only when the user previously had a *global* `core.hooksPath`, forwarding wrappers to that previous global path. There is no fallback to the per-repository `.git/hooks` directory anywhere in the generated scripts.

Consequence: after `diddo init`, every hook a user has in `.git/hooks` in every repo silently stops firing. The most common casualty is `git lfs install`, which writes `pre-push`, `post-checkout`, `post-commit`, and `post-merge` into `.git/hooks` — LFS uploads silently stop. The README claim "Preserves and forwards any previously configured global hooks so existing hooks keep running" is true only for the previous-*global*-path case.

The fix: every hook in the managed directory (all 22 names, now always created) must invoke the repo-local `$GIT_DIR/hooks/<name>` if it exists and is executable — but *only* in the case where no previous global `core.hooksPath` was recorded. See the exclusivity rule in Step 2: git treats `core.hooksPath` as a replacement for `.git/hooks`, so forwarding to both would run hooks that git itself would not have.

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
- `install_local_hook` / the Husky path (lines 465–504) — its *behavior* must not change. The one exception, required by Step 2.4, is the mechanical call-site update `build_post_commit_script(None)` → `build_post_commit_script(None, false)` to match the new signature. That single argument is the only edit permitted in that function.
- `build_post_commit_script_with_previous` (lines 506–532) — leave entirely untouched.
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

**The forwarding rule is EXCLUSIVE, not additive — read this before writing any code.**
Git treats `core.hooksPath` as a *replacement* for `$GIT_DIR/hooks`, never an addition: when a
hooks path is set, git does not look in `.git/hooks` at all. The managed hook must reproduce
exactly what git would have run without diddo, which means precisely one of the two blocks is
emitted, never both:

| Situation at `diddo init` time | What git ran before diddo | What the managed hook must run |
|---|---|---|
| No previous global `core.hooksPath` (common) | `$GIT_DIR/hooks/<name>` | the **local** block only |
| A previous global `core.hooksPath` existed | `<previous>/<name>` only; `.git/hooks` was already dormant | the **previous** block only |

Emitting both would resurrect repo-local hooks that the user's own global `core.hooksPath`
had deliberately bypassed — a stale or broken `.git/hooks/pre-push` from years ago would
suddenly start firing. Do not do it, however reasonable "preserve everything" sounds.

1. `build_forwarding_hook_script(previous_hooks_path: Option<&str>, hook_name: &str)` — change the first parameter to `Option<&str>`. Script layout: shebang + `set -eu`, then **either** the existing previous-hook block (when `Some`) **or** the new local-hook block from Step 1 (when `None`). A plain `match`/`if let ... else` — never both arms.
2. `build_post_commit_script(previous_hooks_path: Option<&str>, include_local_fallback: bool)` — when `previous_hooks_path` is `Some`, keep today's behavior unchanged (previous block, no local block). Only when `previous_hooks_path` is `None` **and** `include_local_fallback` is `true`, insert the local-hook block before the final `diddo_status` exit check, in status-recording form (`set -u` only, no `-e`, matching the existing style):

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

3. **Recursion guard**: none needed beyond the design — the managed dir never lives inside a repo's `$GIT_DIR`, and `--git-dir` ignores `core.hooksPath`.

4. `install_local_hook` (line 497) calls `build_post_commit_script(None)` for the Husky case, and that call site must NOT gain the local fallback: it writes into a repo-local hooks dir that the repo's own `core.hooksPath` points at, so `$GIT_DIR/hooks/post-commit` there is a bypassed, possibly stale hook — the same exclusivity rule. That is what the `include_local_fallback` boolean is for: pass `true` from `install_with`, `false` from `install_local_hook`. Leave `build_post_commit_script_with_previous` (line 506) entirely untouched — it already covers a previous-hook case, so by the exclusivity rule it gets no local block.

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

Three of these six exist to lock in the exclusivity rule (2, 4, 5) — a change that emitted both
blocks would still pass 1 and 3, so do not skip them.

1. `forwarding_script_without_previous_contains_local_fallback` — `build_forwarding_hook_script(None, "pre-push")` contains `git rev-parse --git-dir` and `hooks/$local_hook_name` (or the exact emitted line), and does NOT contain `previous_hook_path`.
2. `forwarding_script_with_previous_omits_local_fallback` — `build_forwarding_hook_script(Some("/prev/hooks"), "pre-push")` contains `previous_hook_path` and does NOT contain `rev-parse --git-dir`.
3. `post_commit_script_contains_local_fallback_for_global_install` — `build_post_commit_script(None, true)` contains the local block and the `local_status` exit propagation.
4. `post_commit_script_with_previous_omits_local_fallback` — `build_post_commit_script(Some("/prev/hooks"), true)` does NOT contain `rev-parse --git-dir`.
5. `local_install_post_commit_has_no_local_fallback` — `build_post_commit_script(None, false)` does NOT contain `git rev-parse --git-dir`. **This is not sufficient on its own**: it pins the builder's behavior but nothing about how `install_local_hook` calls it. Also add an assertion to the *existing* `install_local_hook_adds_post_commit_to_repo_with_local_hooks_path` test (it already reads the written script) that the written script does NOT contain `rev-parse --git-dir`. Without that, flipping `install_local_hook`'s argument to `true` breaks the exclusivity rule with zero test failures — verified by mutation testing during review.
6. `fresh_install_creates_all_hook_wrappers` — via `install_with` with mocked closures: managed dir contains every name in `HOOK_NAMES`.

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

In the "What `diddo init` does" list (README.md ~lines 77–82), replace the preserve/forward bullet with wording that states the exclusive rule accurately. Do not claim both run. Suggested:

- Keeps your existing hooks running: forwards to your previously configured global hooks if you had any, otherwise to each repository's own `.git/hooks` (so repo-local hooks like git-lfs keep working) — matching what git itself would have run.

Read the surrounding bullets first and match their voice and length.

**Verify**: `grep -n '.git/hooks' README.md` shows the new bullet.

## Test plan

Covered in Step 4 (6 new tests + updated assertions) and the Step 5 sandbox check. Model new tests after the existing script-content tests in `src/init.rs`'s test module.

## Done criteria

- [ ] `cargo test` → 0 failed, including the 6 new tests
- [ ] `cargo clippy -- -D warnings` and `cargo fmt -- --check` exit 0
- [ ] `grep -c 'rev-parse --git-dir' src/init.rs` ≥ 1 (the helper emits it; a shared helper legitimately appears once in non-test code)
- [ ] Exclusivity holds: no generated script contains both `previous_hook_path` and `rev-parse --git-dir` (tests 2 and 4 cover this)
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
- Behavior change for release notes: in repos where the user had no previous global `core.hooksPath`, hooks in `.git/hooks` that were silently dead since `diddo init` will start running again; a user with a broken `.git/hooks/pre-push` will now see it fire. Users who had a previous global hooks path see no change. Worth a CHANGELOG line.
- The exclusivity rule (previous-global XOR repo-local) is the load-bearing invariant here. Any future change that makes forwarding additive re-introduces the resurrect-dormant-hooks problem — tests 2 and 4 in the test module exist to catch that.
- If a future feature adds new hook names to `HOOK_NAMES`, the forwarding + local-fallback structure picks them up automatically — reviewer should confirm no hook name is special-cased except `post-commit`.
- Deferred: `uninstall` still deletes the managed dir wholesale; unaffected by this plan.

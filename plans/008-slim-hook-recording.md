# Plan 008: Slim the per-commit hook — subject-only messages, fewer git spawns, locale-proof stat parsing

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 7a8b4ca..HEAD -- src/hook.rs`
> If `src/hook.rs` changed since this plan was written, compare the excerpts
> below before proceeding; on a mismatch, STOP.

## Status

- **Priority**: P2
- **Effort**: M
- **Risk**: LOW-MED (rewrites the test mocks; changes what future rows store for `message`)
- **Depends on**: none (plan 004 touches only src/db.rs — no conflict)
- **Category**: bug + perf
- **Planned at**: commit `7a8b4ca`, 2026-09-05

## Why this matters

`diddo hook` runs on **every git commit the user makes anywhere**. Three problems:

1. **It stores the full raw commit body** (`git log -1 --format=%B`): subject + blank line + body + trailers, with embedded newlines stored verbatim. Every consumer assumes one line — the terminal listing (`{hash}  {message}`), the markdown bullet (`- \`{hash}\` {message}`), and the AI prompt's numbered `1. [repo] message (hash) ...` entries — so any commit with a body (e.g. one carrying `Co-Authored-By:` trailers) renders as a multi-line blob, breaks the markdown list, and corrupts the prompt's enumeration.
2. **Seven serial subprocess spawns per commit** (six `git` calls + one `git show`): roughly 70–140 ms of pure fork/exec overhead added to every commit. Three of them are the same `git log -1` with different formats.
3. **`parse_diff_stats` matches English substrings** (`"files changed"`, `"insertion"`, `"deletion"`) but git localizes `--shortstat` output and the invocation doesn't pin the locale — a non-English `LANG` records `0/0/0` for every commit, forever, silently (`.unwrap_or((0, 0, 0))`). The function also has zero direct tests.

## Current state

- `src/hook.rs` (302 lines total). `build_commit` (lines 21–60):

```rust
    let hash = trim_git_output(run_git(&["rev-parse", "--short", "HEAD"])?);
    let message = trim_git_output(run_git(&["log", "-1", "--format=%B"])?);
    let committed_at =
        parse_git_timestamp(&trim_git_output(run_git(&["log", "-1", "--format=%cI"])?))?;
    let repo_path = trim_git_output(run_git(&["rev-parse", "--show-toplevel"])?);
    let branch = normalize_branch_name(&trim_git_output(run_git(&[
        "rev-parse", "--abbrev-ref", "HEAD",
    ])?));
    ...
    let author_email = run_git(&["config", "user.email"]).ok()...;
```

- `run_git_command` (lines 62–78): `Command::new("git").args(args).output()?`, stdout via `from_utf8_lossy`.
- `read_diff_stats` (lines 80–82): `run_git_command(&["show", "--shortstat", "--format=", "HEAD"])`.
- `parse_diff_stats` (lines 84–111): finds the last line containing `file changed`/`files changed`, splits on commas, parses leading numbers.
- Tests (lines 140–302): 7 tests driving `run_with`/`build_commit` with a **six-arm `match args`** mock closure (e.g. lines 156–165) and a fixed shortstat string. `parse_diff_stats` is never called directly by any test.
- Verified empirically during the audit: `git show --shortstat --format= HEAD` **does** emit first-parent stats for merge commits on current git — do not add merge special-casing.
- **Design constraint — keep `git config user.email`**: summaries group by *profile* = the configured `user.email` (see `src/summary_group.rs` and README "Summaries are grouped by git profile (user.email)"). Do NOT switch to `%ae` (commit author email) — that would change grouping semantics for rebases/cherry-picks. Keep the separate `config user.email` call.
- The DB `message` column is TEXT; old multi-line rows remain and are not migrated (renderer hardening for legacy rows is Plan 015's sanitize step).

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Targeted tests | `cargo test hook::` | all pass |
| Full suite | `cargo test` | 0 failed |
| Lint/format | `cargo clippy -- -D warnings && cargo fmt -- --check` | exit 0 |

## Scope

**In scope**:
- `src/hook.rs` only (+ its tests)

**Out of scope**:
- `src/db.rs` (no schema change — `message` stays TEXT).
- Render/prompt hardening for legacy multi-line rows (Plan 015 / Plan 016).
- Storing the commit body in a second column — deliberately not done; if the AI prompt ever wants bodies, that is a schema decision for a future plan.
- `src/main.rs`, `src/ai/`.

## Git workflow

- Branch: `advisor/008-slim-hook-recording`
- Commit style: conventional commits, e.g. `fix: record commit subject only and reduce hook git spawns`. No AI attribution.
- Do NOT push or open a PR unless instructed.

## Steps

### Step 1: Pin the locale on every git invocation

In `run_git_command`, add `.env("LC_ALL", "C")` to the `Command` builder:

```rust
    let output = Command::new("git").args(args).env("LC_ALL", "C").output()?;
```

This guarantees `--shortstat` emits the English strings `parse_diff_stats` matches. (It also stabilizes any future parsed output.)

**Verify**: `cargo test hook::` still passes (mocks bypass the real command).

### Step 2: Collapse the three `git log` calls and the two `rev-parse` calls

Replace the five metadata calls in `build_commit` with two:

1. One `git log -1 --format=%h%x00%cI%x00%s` — NUL-separated short hash, committer date (strict ISO), **subject only** (`%s` replaces `%B`; this is the message fix). Split the output on `'\0'`, expect exactly 3 fields (trim each with the existing `trim_git_output`). If the split yields fewer than 3 fields, return an `io::Error` naming the command — do not guess.
2. One `git rev-parse --show-toplevel --abbrev-ref HEAD` — returns two lines: toplevel path, then branch name. Split on newline; expect exactly 2 non-empty lines; error otherwise. Feed line 2 through the existing `normalize_branch_name`.

Keep `git config user.email` (see design constraint) and `read_diff_stats` as-is. Net: 7 spawns → 4.

Why `%x00` (NUL) as separator: `%s` cannot contain NUL, and NUL cannot appear in the other fields either, so the split is unambiguous — a subject containing `|` or tabs stays intact.

**Verify**: `cargo build` exits 0 (tests will fail until Step 3 updates the mocks — that is expected; do not run the suite yet).

### Step 3: Update the test mocks

Every `match args` mock in the test module (7 tests) currently has six arms. Rewrite to the new three-command shape, e.g.:

```rust
            |args| match args {
                ["log", "-1", "--format=%h%x00%cI%x00%s"] => {
                    Ok("abc1234\x002026-03-10T12:00:00+00:00\x00feat: add hook storage\n".to_string())
                }
                ["rev-parse", "--show-toplevel", "--abbrev-ref", "HEAD"] => {
                    Ok("/Users/example/projects/diddo\nfeature/diddo\n".to_string())
                }
                ["config", "user.email"] => Ok("work@company.com\n".to_string()),
                _ => Err(io::Error::other("unexpected git arguments")),
            },
```

Match the exact format string you used in Step 2. Keep every existing assertion (hash, message, repo_path, repo_name, branch, stats, committed_at, author_email) — they define the contract and must pass unchanged except that `message` fixtures move into the combined line.

**Verify**: `cargo test hook::` → all 7 existing tests pass.

### Step 4: Direct table-driven tests for `parse_diff_stats`

Add a test (make `parse_diff_stats` visible to the test module — it already is, same file) covering:

| input | expected |
|---|---|
| `" 2 files changed, 10 insertions(+), 3 deletions(-)\n"` | `Some((2, 10, 3))` |
| `" 1 file changed, 1 insertion(+)\n"` | `Some((1, 1, 0))` |
| `" 3 files changed, 12 deletions(-)\n"` | `Some((3, 0, 12))` |
| `""` (empty — e.g. `--allow-empty` commit) | `None` |
| `"some unrelated line\n"` | `None` |
| `" 2 Dateien geändert, 10 Einfügungen(+)\n"` (localized) | `None` — documents WHY Step 1 pins LC_ALL=C |

Also add one `build_commit` test: subject extraction from the combined format when the mock returns a subject containing special characters (e.g. `"fix: handle | pipes & \"quotes\""`) — asserts the NUL split keeps it intact.

Additionally add a malformed-output test: the combined `log` mock returns a string with only 2 NUL-separated fields → `build_commit` returns `Err`.

**Verify**: `cargo test hook::` → all pass, including the new tests.

### Step 5: End-to-end sanity in a temp repo

```sh
tmp=$(mktemp -d) && cd "$tmp" && git init -q && git commit -q --allow-empty -m "subject line" -m "body paragraph"
cd - >/dev/null
```

Then from the diddo repo, run the real command sequence manually against that repo to confirm `%s` yields only `subject line`:

```sh
git -C "$tmp" log -1 --format=%h%x00%cI%x00%s | cat -v
```

Expected: one line, `^@`-separated (NUL shown as `^@` by `cat -v`), message field is exactly `subject line` with no body. Clean up `$tmp`.

**Verify**: output shape as described.

## Test plan

Steps 3–4: 7 updated + ~8 new test cases; the table-driven `parse_diff_stats` test is the regression net for the locale bug. Full gate: `cargo test` 0 failed.

## Done criteria

- [ ] `grep -c '%B' src/hook.rs` → 0; `grep -c '%s' src/hook.rs` ≥ 1
- [ ] `grep -c 'LC_ALL' src/hook.rs` → 1
- [ ] Exactly 4 distinct git invocations in `build_commit`+`read_diff_stats` (count the `run_git`/`run_git_command` call sites)
- [ ] `cargo test` 0 failed; `cargo clippy -- -D warnings`; `cargo fmt -- --check` exit 0
- [ ] Step 5 sanity output confirmed
- [ ] `git diff --name-only` ⊆ {src/hook.rs, plans/README.md}

## STOP conditions

Stop and report back if:

- `build_commit` no longer matches the excerpt (drift).
- The NUL separator does not survive the `io::Result<String>` plumbing (it will — Rust `String` holds NUL fine — but if `from_utf8_lossy` output ever mangles it, report rather than switching to a printable separator silently; a printable separator can collide with subject content).
- Any existing assertion has to *change its expected value* (other than moving fixtures into the combined format) — that means behavior drifted, not just plumbing.

## Maintenance notes

- Rows written before this change may still contain multi-line messages; Plans 015/016 add render/prompt sanitization that covers them. If neither has landed, note in review that legacy rows still render multi-line.
- If profile grouping ever switches from config email to commit author, that is the moment to fold `%ae`/`%ce` into the combined `log` format and drop the `config user.email` spawn (getting to 3 spawns).
- Reviewer should scrutinize: error handling for short splits (must be `Err`, not a silent partial commit record).

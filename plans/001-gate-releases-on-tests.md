# Plan 001: Gate every release build on a green test suite

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 7a8b4ca..HEAD -- .github/workflows/`
> If any in-scope file changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: S
- **Risk**: LOW
- **Depends on**: none
- **Category**: dx (release safety)
- **Planned at**: commit `7a8b4ca`, 2026-09-05

## Why this matters

The test workflow triggers only on `pull_request`, and the release workflow (tag push → multi-platform builds → GitHub Release → Homebrew formula update) contains no test step and no dependency on the test workflow. Every `cargo release` commit lands directly on `main` untested, and a tagged release ships binaries to all users — including via `diddo update` self-update — with zero test execution. The repo has 227 fast tests (`cargo test` finishes in ~0.03s after build); they just aren't wired to the path that matters. This plan is a prerequisite for every riskier plan in this directory: after it lands, a red suite blocks releases.

## Current state

- `.github/workflows/test.yml` — runs `cargo test`, `cargo fmt -- --check`, `cargo clippy -- -D warnings` on `ubuntu-22.04`. Triggers (lines 3–5):

```yaml
on:
  pull_request:
  workflow_dispatch:
```

- `.github/workflows/release.yml` — triggers on `push: tags: ['v*']` and `workflow_dispatch`. Jobs: `build-macos` (line 17), `build-linux` (line 60), `build-windows` (line 107), `release` (line 143, `needs: [build-macos, build-linux, build-windows]`), `update-homebrew` (line 213, `needs: [release]`). No job runs `cargo test`.

- Release flow per `AGENTS.md`: `cargo release patch` creates the version commit and tag; pushing the tag triggers `release.yml`.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Tests | `cargo test` | `test result: ok. 227 passed` (count may have grown) |
| YAML syntax check | `ruby -ryaml -e 'YAML.load_file(".github/workflows/test.yml"); YAML.load_file(".github/workflows/release.yml"); puts "ok"'` | prints `ok` |

(Ruby ships with macOS. If `ruby` is unavailable, use `python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/test.yml')); yaml.safe_load(open('.github/workflows/release.yml')); print('ok')"`; if neither works, careful visual review of indentation is acceptable — note it in your report.)

## Scope

**In scope** (the only files you should modify):
- `.github/workflows/test.yml`
- `.github/workflows/release.yml`

**Out of scope** (do NOT touch, even though they look related):
- Action version pinning, `permissions:` blocks, `${{ inputs.version }}` handling — that is Plan 002; do not do it here even though you will be editing the same file.
- Adding build caching (`Swatinem/rust-cache`) — deliberately deferred; keep this change minimal.
- `Cargo.toml` release metadata, `cliff.toml`.

## Git workflow

- Branch: `advisor/001-gate-releases-on-tests`
- Commit style: conventional commits, e.g. `ci: run tests on main pushes and gate releases on the test suite` (matches history: `chore: add GitHub Actions workflow for testing`). Do not add any AI attribution to commit messages.
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Trigger the test workflow on pushes to main

In `.github/workflows/test.yml`, extend the `on:` block (currently lines 3–5) to:

```yaml
on:
  push:
    branches: [main]
  pull_request:
  workflow_dispatch:
```

**Verify**: the ruby/python YAML check above prints `ok`.

### Step 2: Add a test job to release.yml and gate the builds on it

In `.github/workflows/release.yml`, add a first job named `test` (place it directly under `jobs:`, before `build-macos`), mirroring the steps of `test.yml`:

```yaml
  test:
    runs-on: ubuntu-22.04
    steps:
      - name: Checkout
        uses: actions/checkout@v6

      - name: Install Rust
        uses: dtolnay/rust-toolchain@stable

      - name: Run tests
        run: cargo test --locked

      - name: Check formatting
        run: cargo fmt -- --check

      - name: Clippy
        run: cargo clippy -- -D warnings
```

Note `--locked` on `cargo test`: the release path must build exactly what `Cargo.lock` records; a drifted lockfile should fail the release. (Do not add `--locked` to `test.yml` in this plan.)

Then change the three build jobs to depend on it:

- `build-macos:` → add `needs: [test]`
- `build-linux:` → add `needs: [test]`
- `build-windows:` → add `needs: [test]`

Leave `release` (`needs: [build-macos, build-linux, build-windows]`) and `update-homebrew` (`needs: [release]`) unchanged — they are already transitively gated.

**Verify**: YAML check prints `ok`, and `grep -c 'needs: \[test\]' .github/workflows/release.yml` prints `3`.

### Step 3: Confirm the local suite is green

**Verify**: `cargo test` → `test result: ok.` with 0 failed. Also run `cargo fmt -- --check` (exit 0) and `cargo clippy -- -D warnings` (exit 0) — the new release gate runs these, so confirm they pass at HEAD.

## Test plan

No Rust tests change. The verification is the YAML checks above plus, after merge, observing one green `Test` run on a `main` push and one release run where builds wait on `test` (operator does this; note it in your report as a follow-up for the operator).

## Done criteria

Machine-checkable. ALL must hold:

- [ ] `git diff --name-only` shows only `.github/workflows/test.yml` and `.github/workflows/release.yml` (plus `plans/README.md`)
- [ ] YAML syntax check prints `ok` for both files
- [ ] `grep -c 'needs: \[test\]' .github/workflows/release.yml` → `3`
- [ ] `grep -A2 'on:' .github/workflows/test.yml` shows `push:` with `branches: [main]`
- [ ] `cargo test`, `cargo fmt -- --check`, `cargo clippy -- -D warnings` all exit 0
- [ ] `plans/README.md` status row updated

## STOP conditions

Stop and report back (do not improvise) if:

- The `jobs:` layout of `release.yml` no longer matches the "Current state" description (job names differ or a test job already exists).
- `cargo test --locked` fails locally due to a lockfile mismatch — that means `Cargo.lock` has drifted from `Cargo.toml`; report it, do not run `cargo update`.
- `cargo clippy -- -D warnings` fails at HEAD — the gate would block releases immediately; report the warnings instead of fixing them (they belong to other plans).

## Maintenance notes

- Plan 002 edits the same file (workflow security hardening); execute it after this one to avoid conflicts.
- When a `[profile.release]` or caching change is added later, the new `test` job in `release.yml` is where `--locked` semantics live — keep it in sync with `test.yml` if the steps evolve.
- Reviewer should scrutinize: that `needs: [test]` was added to all three build jobs, not just one.

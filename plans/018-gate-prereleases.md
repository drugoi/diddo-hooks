# Plan 018: Make prerelease tags safe — mark them pre-release and keep them out of Homebrew

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 1483472..HEAD -- .github/workflows/release.yml`
> If `release.yml` changed since this plan was written, compare the
> "Current state" excerpts against the live code before proceeding; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: S
- **Risk**: LOW (adds a guard; the normal-release path is unchanged)
- **Depends on**: plans 001, 002, 005 (all merged — they established the current shape of `release.yml`)
- **Category**: dx (release safety)
- **Planned at**: commit `1483472`, 2026-09-06

## Why this matters

There is currently **no safe way to test the release pipeline.** `release.yml` triggers on any
`v*` tag, and the `Create Release` step sets `draft: false` with no `prerelease:` field, so a
tag like `v0.6.8-rc.1` publishes as a normal release and GitHub marks it **latest**. The
`update-homebrew` job then runs unconditionally (`needs: [release]`, no `if:`) and rewrites the
formula in the **public** `drugoi/homebrew-tap` repository — which currently pins `0.6.7`.

The blast radius of a single RC tag today:

- every `brew upgrade` user is moved onto the release candidate;
- `install.sh` and `install.ps1` resolve the version from `/releases/latest`, so new installs get the RC;
- the background update check offers the RC to every existing user.

That matters right now specifically because plans 001, 002 and 005 rewrote this workflow and
**none of it has ever run** — the first tag is simultaneously the first test of a test gate,
SHA-pinned actions, input validation, and `SHA256SUMS` publication. Testing that with something
that ships to users is the wrong order.

After this plan, a tag whose version contains `-` (the semver prerelease marker: `0.6.8-rc.1`,
`0.7.0-beta.2`) is published as a GitHub pre-release and skips Homebrew entirely. A normal
`v0.6.8` tag behaves exactly as it does today.

## Current state

- `.github/workflows/release.yml`, `release` job — the `Create Release` step (~line 258):

```yaml
      - name: Create Release
        uses: softprops/action-gh-release@3bb12739c298aeb8a4eeaf626c5b8d85266b0e65  # v2
        with:
          tag_name: v${{ steps.version.outputs.version }}
          name: v${{ steps.version.outputs.version }}
          body_path: release_notes.md
          draft: false
          files: release/*
        env:
          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}
```

- The `release` job declares no `outputs:` block. Its `Set version` step has `id: version` and
  writes `version` to `$GITHUB_OUTPUT`.

- `update-homebrew` job header (~line 271):

```yaml
  update-homebrew:
    needs: [release]
    runs-on: ubuntu-latest
    permissions:
      contents: read
    steps:
```

- The workflow triggers on `push: tags: ['v*']` and on `workflow_dispatch` with a required
  `version` input.

**Important — why the guard must NOT use `github.ref_name`.** On a tag push `github.ref_name`
is the tag (`v0.6.8-rc.1`), but on `workflow_dispatch` it is the *branch* (`main`). Gating on
it would silently mis-classify every manual run. Both jobs already compute a validated
`steps.version.outputs.version`; that is the value to test.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| YAML syntax | `ruby -ryaml -e 'YAML.load_file(".github/workflows/release.yml"); puts "ok"'` | `ok` |
| Job graph | `ruby -ryaml -e 'y=YAML.load_file(".github/workflows/release.yml"); y["jobs"].each{\|k,v\| puts "#{k}: needs=#{v["needs"].inspect} if=#{v["if"].inspect}"}'` | prints each job, its needs, and its `if` |

(If `ruby` is unavailable use `python3 -c "import yaml; yaml.safe_load(open('.github/workflows/release.yml')); print('ok')"`.)

## Scope

**In scope**:
- `.github/workflows/release.yml` — the `release` job's `outputs:` block, the `Create Release` step, and the `update-homebrew` job's `if:`
- `README.md` — one line in the release/contributing area noting that `-`-suffixed tags are prereleases (only if such a section already exists; if not, skip and say so)

**Out of scope**:
- Action pins, `permissions:` blocks, the `validate-input` job, `SHA256SUMS` generation — plans 002 and 005 own those; leave every one of them byte-for-byte.
- The `test`/build job graph — plan 001 owns it.
- `cargo release` configuration, `cliff.toml`, `Cargo.toml`.
- Any Rust source file.

## Git workflow

- Branch: `advisor/018-gate-prereleases`
- Commit style: conventional commits, e.g. `ci: publish prerelease tags as pre-releases and skip the Homebrew update`. No AI attribution.
- Do NOT push or open a PR unless instructed.

## Steps

### Step 1: Expose the resolved version as a job output

In the `release` job, add an `outputs:` block so downstream jobs can read the version. Place it
directly under `runs-on:`/`permissions:`, before `steps:`:

```yaml
  release:
    needs: [build-macos, build-linux, build-windows]
    runs-on: ubuntu-latest
    permissions:
      contents: write
    outputs:
      version: ${{ steps.version.outputs.version }}
    steps:
```

Do not change `needs:` or `permissions:`.

**Verify**: YAML check prints `ok`.

### Step 2: Mark `-`-suffixed versions as GitHub pre-releases

In the `Create Release` step, add a `prerelease:` input. Keep every other input unchanged:

```yaml
        with:
          tag_name: v${{ steps.version.outputs.version }}
          name: v${{ steps.version.outputs.version }}
          body_path: release_notes.md
          draft: false
          prerelease: ${{ contains(steps.version.outputs.version, '-') }}
          files: release/*
```

A GitHub release flagged `prerelease: true` is excluded from `/releases/latest`, which is what
`install.sh`, `install.ps1` and the `diddo update` check all resolve against — so an RC becomes
invisible to them without any installer change.

**Verify**: YAML check `ok`; `grep -c 'prerelease:' .github/workflows/release.yml` → `1`.

### Step 3: Skip Homebrew for prereleases

Add an `if:` to the `update-homebrew` job:

```yaml
  update-homebrew:
    needs: [release]
    if: ${{ !contains(needs.release.outputs.version, '-') }}
    runs-on: ubuntu-latest
```

This is the guard that keeps a release candidate out of the public tap. Note it reads
`needs.release.outputs.version` (the Step 1 output), NOT `github.ref_name` — see the warning in
"Current state".

**Verify**: YAML check `ok`; the job-graph command shows `update-homebrew` with a non-nil `if`.

### Step 4: Prove the expressions evaluate correctly

GitHub expression semantics cannot be executed locally, so verify the logic by reasoning it
through explicitly and record the table in your report:

| version | `contains(v, '-')` | release marked | Homebrew runs? |
|---|---|---|---|
| `0.6.8` | false | normal release | yes |
| `0.6.8-rc.1` | true | pre-release | no |
| `0.7.0-beta.2` | true | pre-release | no |

Then confirm mechanically that both expressions reference the same source value:

```sh
grep -n "contains(steps.version.outputs.version, '-')" .github/workflows/release.yml
grep -n "contains(needs.release.outputs.version, '-')" .github/workflows/release.yml
```

Both must return exactly one line. If either references `github.ref_name`, that is a defect —
fix it before reporting.

**Verify**: both greps return one line each; YAML check `ok`.

### Step 5: README note (conditional)

Look for an existing release/contributing/maintainer section in `README.md` (try
`grep -n -i 'cargo release\|releasing\|maintainer' README.md`). If one exists, add one line:
tags with a `-` suffix (e.g. `v0.6.8-rc.1`) publish as GitHub pre-releases and do not update
Homebrew. If no such section exists, **skip this step** and say so in your report — do not
invent a new README section for this.

**Verify**: state in your report whether the section existed.

## Test plan

No Rust tests; nothing here is locally executable. The gates are the YAML/grep checks above
plus the reasoning table in Step 4. The real test is the operator tagging `v0.6.8-rc.1` after
merge and confirming: the GitHub release is badged "Pre-release", `/releases/latest` still
resolves to `v0.6.7`, and the `update-homebrew` job shows as skipped.

## Done criteria

- [ ] YAML check prints `ok`
- [ ] `release` job declares `outputs.version`
- [ ] `grep -c "prerelease: \${{ contains(steps.version.outputs.version, '-') }}" .github/workflows/release.yml` → `1`
- [ ] `update-homebrew` has `if: ${{ !contains(needs.release.outputs.version, '-') }}`
- [ ] Neither new expression references `github.ref_name`
- [ ] Job graph otherwise unchanged: `validate-input → test → {build-macos, build-linux, build-windows} → release → update-homebrew`
- [ ] `git diff --name-only main` ⊆ {`.github/workflows/release.yml`, `README.md`}

## STOP conditions

Stop and report back if:

- The `Create Release` step or `update-homebrew` header no longer matches the excerpts.
- You find an existing `prerelease:` or `if:` on those elements (someone already did this).
- You are tempted to change the tag trigger pattern (`v*`) to exclude prereleases — that is the
  opposite of the goal; prerelease tags must still build and publish, just not as latest and
  not to Homebrew.

## Maintenance notes

- The `-` test is the semver prerelease marker and matches what `validate-input` (plan 002)
  already accepts: its regex permits `0.6.8-rc.1`. The two are intentionally consistent — if
  that regex ever changes, revisit this guard.
- If a future change adds another consumer of releases (a Scoop manifest, an AUR package,
  a container tag), it needs the same `if:` guard, or prereleases will leak into it.
- Reviewer should scrutinize: that the guard reads `needs.release.outputs.version` and not
  `github.ref_name`, because the difference only shows up on `workflow_dispatch` and would
  otherwise be invisible until it misfires.

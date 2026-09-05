# Plan 002: Harden the release workflow — input handling, permissions, action pinning, token hygiene

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 7a8b4ca..HEAD -- .github/workflows/`
> Plan 001 is expected to have edited these files (added a `test` job to
> release.yml and a push trigger to test.yml) — that drift is fine. Any other
> structural change: compare the excerpts below against the live code; on a
> mismatch, treat it as a STOP condition.

## Status

- **Priority**: P1
- **Effort**: S
- **Risk**: LOW
- **Depends on**: plans/001-gate-releases-on-tests.md (same files; do 001 first)
- **Category**: security
- **Planned at**: commit `7a8b4ca`, 2026-09-05

## Why this matters

Three defensive-maintenance gaps in `.github/workflows/release.yml`, a workflow that produces the shipped binaries and holds a Homebrew tap push token:

1. `${{ github.event.inputs.version }}` is expanded by the Actions template engine directly into `run:` script bodies at ~7 sites. Template expansion happens *before* the shell parses the script, so the input becomes script source text, not a quoted variable. Anyone able to trigger `workflow_dispatch` controls script text in jobs with `contents: write` and access to `HOMEBREW_TAP_TOKEN`.
2. The tap token is embedded in a clone URL (`https://x-access-token:${GH_TOKEN}@github.com/...`), which persists into `tap/.git/config` on the runner and appears in git error output paths.
3. All actions are referenced by mutable tags (`actions/checkout@v6`, `dtolnay/rust-toolchain@stable` — a moving branch), and no job except `release` declares `permissions:`, so jobs inherit the repository default token scope.

## Current state

- `.github/workflows/release.yml` — the interpolation pattern, repeated in `build-macos` (~line 32), `build-linux` (~75), `build-windows` (~116, PowerShell), `release` (~152, ~175), `update-homebrew` (~220):

```yaml
      - name: Set version
        id: version
        run: |
          if [ -n "${{ github.event.inputs.version }}" ]; then
            echo "version=${{ github.event.inputs.version }}" >> $GITHUB_OUTPUT
          else
            echo "version=${GITHUB_REF_NAME#v}" >> $GITHUB_OUTPUT
          fi
```

- The token-in-URL clone in `update-homebrew` (~line 242):

```yaml
      - name: Update Homebrew formula
        env:
          GH_TOKEN: ${{ secrets.HOMEBREW_TAP_TOKEN }}
        run: |
          VERSION="${{ steps.version.outputs.version }}"
          git clone https://x-access-token:${GH_TOKEN}@github.com/drugoi/homebrew-tap.git tap
          cd tap
```

- Also in `update-homebrew`, `${{ steps.sha.outputs.* }}` values are interpolated into `sed` command lines (~lines 246–249). These outputs are produced by the workflow's own `sha256sum` step, so they are not attacker-controlled; leave them as-is.
- Action refs in use: `actions/checkout@v6`, `dtolnay/rust-toolchain@stable`, `actions/upload-artifact@v7`, `actions/download-artifact@v8`, `softprops/action-gh-release@v2`.
- `.github/workflows/test.yml` uses `actions/checkout@v6` and `dtolnay/rust-toolchain@stable`.
- Only the `release` job has a `permissions:` block (`contents: write`).

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| YAML syntax | `ruby -ryaml -e 'YAML.load_file(".github/workflows/release.yml"); YAML.load_file(".github/workflows/test.yml"); puts "ok"'` | `ok` |
| Resolve action SHA | `gh api repos/actions/checkout/commits/v6 --jq .sha` | 40-char SHA |
| Interpolation gone | `grep -c 'inputs.version }}"' .github/workflows/release.yml` | see steps |

## Scope

**In scope**:
- `.github/workflows/release.yml`
- `.github/workflows/test.yml` (action pinning + permissions only)

**Out of scope**:
- The `steps.sha.outputs.*` sed interpolations (workflow-internal values; Plan 005 restructures that step anyway).
- Rotating `HOMEBREW_TAP_TOKEN` — an executor cannot do this; it is flagged in the report instead (see Step 5).
- Checksums / SHA256SUMS publication — Plan 005.
- Any Rust source file.

## Git workflow

- Branch: `advisor/002-harden-release-workflow`
- Commit style: conventional commits, e.g. `ci: harden release workflow input handling and permissions`. No AI attribution in commit messages.
- Do NOT push or open a PR unless the operator instructed it.

## Steps

### Step 1: Validate the dispatch input once, fail closed

Add a new first job (before `test` from Plan 001) in `release.yml`:

```yaml
  validate-input:
    runs-on: ubuntu-latest
    steps:
      - name: Validate version input
        env:
          VERSION_INPUT: ${{ github.event.inputs.version }}
        run: |
          if [ "${GITHUB_EVENT_NAME}" = "workflow_dispatch" ]; then
            echo "$VERSION_INPUT" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.-]+)?$' \
              || { echo "Invalid version input: not a semver version" >&2; exit 1; }
          fi
```

Make the `test` job (from Plan 001) `needs: [validate-input]` so everything is transitively gated. Note the pattern: the untrusted value is bound via `env:` and referenced as a shell variable — never `${{ }}`-expanded inside `run:`.

**Verify**: YAML check prints `ok`.

### Step 2: Replace every `${{ github.event.inputs.version }}` inside run: blocks with an env binding

For each `Set version` step (5 occurrences), change to the env-bound form. Bash version:

```yaml
      - name: Set version
        id: version
        env:
          VERSION_INPUT: ${{ github.event.inputs.version }}
        run: |
          if [ -n "$VERSION_INPUT" ]; then
            echo "version=$VERSION_INPUT" >> $GITHUB_OUTPUT
          else
            echo "version=${GITHUB_REF_NAME#v}" >> $GITHUB_OUTPUT
          fi
```

PowerShell version (`build-windows`):

```yaml
      - name: Set version
        id: version
        env:
          VERSION_INPUT: ${{ github.event.inputs.version }}
        run: |
          if ($env:VERSION_INPUT) {
            echo "version=$env:VERSION_INPUT" >> $env:GITHUB_OUTPUT
          } else {
            $ver = $env:GITHUB_REF_NAME.TrimStart('v')
            echo "version=$ver" >> $env:GITHUB_OUTPUT
          }
        shell: pwsh
```

Also fix the `Generate release notes` step in the `release` job, which tests `[ -n "${{ github.event.inputs.version }}" ]` (~line 175) — bind it via the same `env: VERSION_INPUT` and test `[ -n "$VERSION_INPUT" ]`.

After this step the ONLY remaining `${{ github.event.inputs.version }}` occurrences must be inside `env:` blocks.

**Verify**: `grep -n 'inputs.version' .github/workflows/release.yml` — every hit is on a line of the form `VERSION_INPUT: ${{ github.event.inputs.version }}` under an `env:` key. No hit inside a `run:` script body.

### Step 3: Add least-privilege permissions

At the workflow level of `release.yml` (top level, after `env:`), add:

```yaml
permissions:
  contents: read
```

Keep the existing `permissions: contents: write` on the `release` job (it creates the GitHub Release). Add `permissions: contents: read` explicitly to `update-homebrew` (it pushes to a *different* repo using its own PAT, so it needs no scope on this repo). In `test.yml`, add workflow-level `permissions: contents: read`.

**Verify**: YAML check prints `ok`; `grep -c 'permissions:' .github/workflows/release.yml` → at least `3` (workflow level, release job, update-homebrew job).

### Step 4: Pin actions to commit SHAs

For each `uses:` in both workflow files, resolve the current SHA of the tag and pin, keeping the tag as a comment:

```yaml
        uses: actions/checkout@<sha>  # v6
```

Resolve with: `gh api repos/<owner>/<repo>/commits/<tag> --jq .sha` (e.g. `gh api repos/actions/checkout/commits/v6 --jq .sha`). For `dtolnay/rust-toolchain@stable`, pin the *action* to its current master SHA (`gh api repos/dtolnay/rust-toolchain/commits/master --jq .sha`) with comment `# stable toolchain` — the SHA pins the action code, not the Rust version.

Also add `persist-credentials: false` to every `actions/checkout` step in both files (none of them push to this repo; the `update-homebrew` job authenticates to the tap separately):

```yaml
        uses: actions/checkout@<sha>  # v6
        with:
          persist-credentials: false
```

(where a `with:` already exists — e.g. `fetch-depth: 0` in the release job, `targets:` on rust-toolchain — merge into it).

If `gh` is not authenticated or has no network, STOP condition — do not guess SHAs.

**Verify**: `grep -E 'uses: .+@[0-9a-f]{40}' .github/workflows/release.yml .github/workflows/test.yml | wc -l` equals the total number of `uses:` lines in both files (`grep -c 'uses:' <both files>`).

### Step 5: Take the token out of the clone URL

In `update-homebrew`, replace the clone+push with `gh`-authenticated operations (the runner has `gh` preinstalled and `GH_TOKEN` is already exported in that step's `env:`):

```yaml
      - name: Update Homebrew formula
        env:
          GH_TOKEN: ${{ secrets.HOMEBREW_TAP_TOKEN }}
        run: |
          VERSION="${{ steps.version.outputs.version }}"
          gh repo clone drugoi/homebrew-tap tap -- --depth 1
          cd tap
          # ... existing sed lines unchanged ...
          git config user.name "Nikita Bayev"
          git config user.email "nikita@bayev.kz"
          git add Formula/diddo.rb
          git commit -m "Update diddo to ${VERSION}"
          gh auth setup-git
          git push
```

(`gh repo clone` and `gh auth setup-git` use `GH_TOKEN` via a credential helper — the token never appears in a URL, argv, or `.git/config`.) `steps.version.outputs.version` is already validated by Step 1 + produced by the env-bound Step 2, so its interpolation here is acceptable; keep it.

Include in your final report: **the operator should rotate `HOMEBREW_TAP_TOKEN`**, since it has been passed via clone URLs in previously published workflow runs. Also recommend enabling Dependabot for `github-actions` to keep the SHA pins fresh (out of scope to configure here).

**Verify**: `grep -c 'x-access-token' .github/workflows/release.yml` → `0`.

## Test plan

No Rust tests. Verification is the grep/YAML gates above. The first real `workflow_dispatch` release after merge is the end-to-end test — flag in your report that the operator should run one against a prerelease tag.

## Done criteria

- [ ] No `${{ github.event.inputs.version }}` inside any `run:` script body (only under `env:`)
- [ ] `validate-input` job exists and `test` needs it
- [ ] All `uses:` pinned to 40-char SHAs with version comments, in both workflow files
- [ ] `persist-credentials: false` on every checkout
- [ ] Workflow-level `permissions: contents: read` in both files; `x-access-token` absent
- [ ] YAML check `ok`; `git diff --name-only` shows only the two workflow files (plus `plans/README.md`)
- [ ] Final report tells the operator to rotate `HOMEBREW_TAP_TOKEN`

## STOP conditions

Stop and report back if:

- `gh api` cannot resolve an action SHA (no auth/network) — report which pins are missing rather than inventing SHAs.
- The `update-homebrew` job's structure differs from the excerpt (e.g. Plan 005 already restructured the SHA step) — reconcile with the live code only if the change is mechanical; otherwise report.
- Any verification grep returns an unexpected count twice after a fix attempt.

## Maintenance notes

- SHA pins go stale; the report recommends Dependabot (`package-ecosystem: github-actions`) — Plan 005's reviewer may bundle that.
- If a future workflow adds `pull_request_target` or new dispatch inputs, apply the same env-binding rule; the `validate-input` job is the place for new input checks.
- Reviewer should scrutinize: the PowerShell env-binding (quoting differs from bash) and that `gh auth setup-git` precedes `git push`.

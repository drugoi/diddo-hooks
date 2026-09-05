# Plan 005: Publish SHA256SUMS with releases and verify downloads in both installers

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat 7a8b4ca..HEAD -- .github/workflows/release.yml install.sh install.ps1`
> Plans 001/002 are expected to have edited release.yml (test gate, env-bound
> inputs, SHA-pinned actions) — that drift is fine. Any other structural
> change: compare excerpts before proceeding; on a mismatch, STOP.

## Status

- **Priority**: P1
- **Effort**: M
- **Risk**: MED (a verification bug hard-fails installs; the rollout ordering in Step 4 mitigates)
- **Depends on**: plans/001-gate-releases-on-tests.md, plans/002-harden-release-workflow.md (same workflow file)
- **Category**: security
- **Planned at**: commit `7a8b4ca`, 2026-09-05

## Why this matters

The two most-advertised install paths — `curl -sSL .../install.sh | sh` and `irm .../install.ps1 | iex` — download and execute a binary whose only trust anchor is TLS to github.com. No checksum is published as a release asset, and neither installer verifies anything. (The Homebrew formula *does* pin SHA256 per target, so brew users are covered.) Additionally, `install.sh` interpolates the unvalidated `DIDDO_VERSION` env var into the download URL, so a malformed pin silently redirects which asset is fetched, and the script runs `set -e` without `set -u`.

This plan makes the release job publish a `SHA256SUMS` asset computed from the exact artifacts it uploads, and makes both installers (a) validate the version string against a strict pattern and (b) verify the downloaded archive against `SHA256SUMS` before installing. It also switches the Homebrew-formula SHA step to use the published sums instead of re-downloading tarballs.

## Current state

- `.github/workflows/release.yml`, `release` job — collects artifacts and uploads (at `7a8b4ca`, ~lines 194–209):

```yaml
      - name: Collect release assets
        run: |
          mkdir -p release
          for f in artifacts/*.tar.gz artifacts/*.zip; do
            [ -f "$f" ] && cp "$f" "release/$(basename "$f")"
          done
          ls -la release/

      - name: Create Release
        uses: softprops/action-gh-release@v2   # may be SHA-pinned by plan 002
        with:
          ...
          files: release/*
```

- `update-homebrew` job, `Compute SHA256 for release assets` step (~lines 226–235): re-downloads each tarball from the published release with `curl -sL ... | sha256sum` — replace with a parse of the published `SHA256SUMS`.

- `install.sh` (75 lines): `set -e` (line 6, no `-u`); `get_version` (lines 37–49) returns `$DIDDO_VERSION` unvalidated when set; download/extract/mv (lines 61–67):

```sh
VERSION=$(get_version) || exit 1
TARBALL="diddo-${VERSION}-${TARGET}.tar.gz"
URL="${BASE_URL}/releases/download/v${VERSION}/${TARBALL}"
...
curl -sSL -o "${tmpdir}/${TARBALL}" "$URL"
tar -xzf "${tmpdir}/${TARBALL}" -C "$tmpdir"
mv "$tmpdir/diddo" "${INSTALL_DIR}/diddo"
```

- `install.ps1` (75 lines): `Get-Version` (lines 23–44) returns `$env:DIDDO_VERSION` unvalidated; `Invoke-WebRequest` + `Expand-Archive` (lines 55–58); the `try/catch` in `Get-Version` swallows all non-302 failures and falls through to the API call.

- Portability constraint: macOS has `shasum -a 256`, most Linux has `sha256sum`. The installer must handle both.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Shell syntax | `sh -n install.sh` | exit 0, no output |
| Shellcheck (if installed) | `shellcheck install.sh` | no errors (warnings acceptable — report them) |
| PowerShell syntax (if pwsh installed) | `pwsh -NoProfile -Command "[void][System.Management.Automation.Language.Parser]::ParseFile('install.ps1', [ref]$null, [ref]$errs); $errs.Count"` | prints `0` |
| YAML syntax | `ruby -ryaml -e 'YAML.load_file(".github/workflows/release.yml"); puts "ok"'` | `ok` |

If `pwsh` is not installed, skip its check and note that in your report; the ps1 change is small enough for careful review.

## Scope

**In scope**:
- `.github/workflows/release.yml` (release + update-homebrew jobs)
- `install.sh`
- `install.ps1`
- `README.md` — one sentence noting installers verify checksums (Install section)

**Out of scope**:
- Checksum verification inside `diddo update` (`src/update.rs`) — deliberately deferred: it is coupled to the decision about replacing the `self_update` crate (audit finding: `self_update 0.36` pulls in an EOL rustls 0.21 stack). Recorded in `plans/README.md` as deferred; do not attempt here.
- Signing (minisign/cosign) — follow-up, not this plan.
- Any Rust source file.

## Git workflow

- Branch: `advisor/005-verify-release-downloads`
- Commit style: conventional commits, e.g. `feat: publish SHA256SUMS and verify installer downloads`. No AI attribution.
- Do NOT push or open a PR unless instructed.

## Steps

### Step 1: Generate SHA256SUMS in the release job

In the `Collect release assets` step of `release.yml`, after the copy loop, add:

```sh
          (cd release && sha256sum *.tar.gz *.zip > SHA256SUMS)
          cat release/SHA256SUMS
```

`files: release/*` already uploads everything in that directory, so `SHA256SUMS` becomes a release asset with no further change. Note: sums are computed from the artifacts the job is about to upload — same provenance as the binaries, unlike the current re-download approach.

**Verify**: YAML check `ok`.

### Step 2: Use the published sums in update-homebrew

Replace the body of `Compute SHA256 for release assets` with a download of `SHA256SUMS` and a parse (keep the same `$GITHUB_OUTPUT` keys so the `sed` lines are untouched):

```sh
          VERSION="${{ steps.version.outputs.version }}"
          BASE="https://github.com/drugoi/diddo-hooks/releases/download/v${VERSION}"
          curl -fsSL -o SHA256SUMS "${BASE}/SHA256SUMS"
          for target in aarch64-apple-darwin x86_64-apple-darwin aarch64-unknown-linux-gnu x86_64-unknown-linux-gnu; do
            hash=$(awk -v f="diddo-${VERSION}-${target}.tar.gz" '$2 == f {print $1}' SHA256SUMS)
            [ -n "$hash" ] || { echo "No checksum for ${target}" >&2; exit 1; }
            key=$(echo "$target" | tr '-' '_')
            echo "${key}=${hash}" >> $GITHUB_OUTPUT
          done
```

(If Plan 002 landed, `steps.version.outputs.version` is already produced from a validated, env-bound input — safe to interpolate.) Note `-f` on curl: a missing asset must fail the job, not produce an HTML error page.

**Verify**: YAML check `ok`; `grep -c 'curl -sL' .github/workflows/release.yml` → 0 in that step (the old per-tarball downloads are gone).

### Step 3: Verify in install.sh

1. Line 6: change `set -e` → `set -eu`.
2. In `get_version`, validate before returning (both the env-var and the resolved-latest paths):

```sh
validate_version() {
  echo "$1" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.-]+)?$' || {
    echo "Invalid version '$1' (expected e.g. 0.6.7)" >&2
    return 1
  }
}
```

Call `validate_version "$DIDDO_VERSION"` before echoing it, and `validate_version "${tag#v}"` for the resolved tag. Because `set -u` is now active, guard the env read: `if [ -n "${DIDDO_VERSION:-}" ]; then ...`.

3. After the download, before `tar -xzf`, verify:

```sh
sha256_tool() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{print $1}'
  else shasum -a 256 "$1" | awk '{print $1}'; fi
}

SUMS_URL="${BASE_URL}/releases/download/v${VERSION}/SHA256SUMS"
if curl -fsSL -o "${tmpdir}/SHA256SUMS" "$SUMS_URL"; then
  expected=$(awk -v f="$TARBALL" '$2 == f {print $1}' "${tmpdir}/SHA256SUMS")
  if [ -z "$expected" ]; then
    echo "Release v${VERSION} has no checksum entry for ${TARBALL}; aborting." >&2
    exit 1
  fi
  actual=$(sha256_tool "${tmpdir}/${TARBALL}")
  if [ "$actual" != "$expected" ]; then
    echo "Checksum mismatch for ${TARBALL}: expected ${expected}, got ${actual}. Aborting." >&2
    exit 1
  fi
  echo "Checksum verified."
else
  if [ "${DIDDO_SKIP_CHECKSUM:-}" = "1" ]; then
    echo "WARNING: no SHA256SUMS published for v${VERSION}; skipping verification (DIDDO_SKIP_CHECKSUM=1)." >&2
  else
    echo "Release v${VERSION} does not publish SHA256SUMS (older release?)." >&2
    echo "Set DIDDO_SKIP_CHECKSUM=1 to install anyway, or pin a newer version." >&2
    exit 1
  fi
fi
```

Rationale for the escape hatch: `DIDDO_VERSION` pinning of pre-checksum releases is documented in the README; failing closed with an explicit override keeps the security default without breaking that documented use.

**Verify**: `sh -n install.sh` exits 0. Then a dry functional check against the *latest existing* release (which has no SHA256SUMS yet): `DIDDO_INSTALL_DIR=$(mktemp -d) sh install.sh` → must exit 1 with the "does not publish SHA256SUMS" message; and `DIDDO_SKIP_CHECKSUM=1 DIDDO_INSTALL_DIR=$(mktemp -d) sh install.sh` → installs with the warning. Also `DIDDO_VERSION='bogus/../path' sh install.sh` → exits 1 with "Invalid version".

### Step 4: Verify in install.ps1

Mirror the logic:

1. In `Get-Version`, validate: `if ($v -notmatch '^[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.-]+)?$') { Write-Error "Invalid version '$v'" }` for both the env-var and resolved-tag paths. In the `try/catch`, re-throw unexpected exceptions: keep the 302 handling, but add `else { throw }` so real failures aren't silently swallowed.
2. After downloading the zip, fetch `"$BaseUrl/releases/download/v$Version/SHA256SUMS"` into a temp file; if the request fails: honor `$env:DIDDO_SKIP_CHECKSUM -eq "1"` with a warning, else `Write-Error` (same policy as install.sh). If it succeeds: find the line ending in the zip name, compare against `(Get-FileHash -Algorithm SHA256 $TempZip).Hash.ToLower()`, `Write-Error` on mismatch.

**Verify**: the pwsh parser check prints `0` (or skipped-with-note if pwsh unavailable).

### Step 5: README note

In the Install section, after the curl/irm commands, add one line: installers verify the download against the release's `SHA256SUMS`; pinning a version older than the first checksummed release requires `DIDDO_SKIP_CHECKSUM=1`.

**Verify**: `grep -c 'SHA256SUMS' README.md` ≥ 1.

## Test plan

No Rust tests. Functional gates: the three install.sh invocations in Step 3's verify block (missing-sums failure, skip-var success, bogus-version rejection). Report to the operator: the first release after merge should be a prerelease tag to confirm the `SHA256SUMS` asset appears and `update-homebrew` reads it.

## Done criteria

- [ ] `sh -n install.sh` exit 0; the three functional invocations behave as specified
- [ ] `SHA256SUMS` generated in the `Collect release assets` step and consumed by `update-homebrew`
- [ ] Version validation present in both installers (`grep -c 'Invalid version' install.sh install.ps1` → ≥1 each)
- [ ] `set -eu` in install.sh
- [ ] YAML check `ok`; `git diff --name-only` ⊆ {release.yml, install.sh, install.ps1, README.md, plans/README.md}
- [ ] `plans/README.md` status row updated (and the deferred `diddo update` verification noted there)

## STOP conditions

Stop and report back if:

- The `Collect release assets` / `Compute SHA256` steps no longer match the excerpts (a later plan restructured them differently than 001/002 predict).
- `sh -n` or the functional checks fail twice after a fix attempt.
- You are tempted to add checksum logic to `src/update.rs` — that is explicitly out of scope; report the temptation instead.

## Maintenance notes

- The first release after this merges is the compatibility boundary: older pinned versions need `DIDDO_SKIP_CHECKSUM=1`. Consider noting the boundary version in the README once known.
- Follow-ups deferred: checksum/signature verification in `diddo update` (couple it to the `self_update`-replacement decision); minisign/cosign signature over SHA256SUMS.
- Reviewer should scrutinize: awk field matching (`$2 == f`) — `sha256sum` output separates with two spaces; `awk` default splitting handles it, but a `*` binary-mode marker (`*filename`) would not match — the generation step uses text mode, keep it that way.

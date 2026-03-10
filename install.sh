#!/bin/sh
# Install diddo from GitHub Releases.
# Usage: curl -sSL https://raw.githubusercontent.com/drugoi/diddo-hooks/main/install.sh | sh
# Pin version: DIDDO_VERSION=0.1.0 curl -sSL ... | sh

set -e

REPO="drugoi/diddo-hooks"
BASE_URL="https://github.com/${REPO}"
INSTALL_DIR="${DIDDO_INSTALL_DIR:-$HOME/.local/bin}"

# Detect OS and arch for release asset name (same as Rust target triple).
detect_target() {
  OS=$(uname -s)
  ARCH=$(uname -m)
  case "$OS" in
    Darwin)
      case "$ARCH" in
        arm64) echo "aarch64-apple-darwin" ;;
        x86_64) echo "x86_64-apple-darwin" ;;
        *) echo "unsupported"; return 1 ;;
      esac ;;
    Linux)
      case "$ARCH" in
        x86_64) echo "x86_64-unknown-linux-gnu" ;;
        aarch64|arm64) echo "aarch64-unknown-linux-gnu" ;;
        *) echo "unsupported"; return 1 ;;
      esac ;;
    *)
      echo "unsupported"
      return 1
      ;;
  esac
}

# Resolve version: DIDDO_VERSION env, or latest from GitHub API.
get_version() {
  if [ -n "$DIDDO_VERSION" ]; then
    echo "$DIDDO_VERSION"
    return
  fi
  # Follow redirect from /releases/latest to get tag, then strip 'v' prefix.
  tag=$(curl -sSL -o /dev/null -w '%{url_effective}' "${BASE_URL}/releases/latest" | sed -n 's|.*/tag/||p')
  if [ -z "$tag" ]; then
    echo "Could not determine latest release. Set DIDDO_VERSION=0.1.0 or create a release on GitHub." >&2
    return 1
  fi
  echo "${tag#v}"
}

TARGET=$(detect_target) || exit 1
if [ "$TARGET" = "unsupported" ]; then
  echo "Unsupported platform: $(uname -s) $(uname -m). macOS (Apple Silicon or Intel) and Linux (x86_64, aarch64) are supported." >&2
  exit 1
fi

VERSION=$(get_version) || exit 1
TARBALL="diddo-${VERSION}-${TARGET}.tar.gz"
URL="${BASE_URL}/releases/download/v${VERSION}/${TARBALL}"

mkdir -p "$INSTALL_DIR"
tmpdir=$(mktemp -d)
trap 'rm -rf "$tmpdir"' EXIT
curl -sSL -o "${tmpdir}/${TARBALL}" "$URL"
tar -xzf "${tmpdir}/${TARBALL}" -C "$tmpdir"
mv "$tmpdir/diddo" "${INSTALL_DIR}/diddo"
chmod +x "${INSTALL_DIR}/diddo"

echo "Installed diddo ${VERSION} to ${INSTALL_DIR}/diddo"
if ! command -v diddo >/dev/null 2>&1; then
  echo "Add ${INSTALL_DIR} to your PATH, for example:"
  echo "  echo 'export PATH=\"\$HOME/.local/bin:\$PATH\"' >> ~/.profile"
  echo "  echo 'export PATH=\"\$HOME/.local/bin:\$PATH\"' >> ~/.zshrc  # or ~/.bashrc"
fi

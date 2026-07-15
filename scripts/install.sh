#!/usr/bin/env bash
# Install the latest prebuilt AideMemo CLI from GitHub Releases.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/taeyun16/aidememo/main/scripts/install.sh | bash
#
# Overrides:
#   AIDEMEMO_VERSION=v0.1.0
#   AIDEMEMO_INSTALL_DIR=$HOME/.local/bin

set -euo pipefail

REPO="${AIDEMEMO_REPO:-taeyun16/aidememo}"
INSTALL_DIR="${AIDEMEMO_INSTALL_DIR:-$HOME/.local/bin}"

require() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "error: $1 is required" >&2
        exit 1
    fi
}

require curl
require tar

case "$(uname -s)" in
    Darwin) os="apple-darwin" ;;
    Linux) os="unknown-linux-gnu" ;;
    *)
        echo "error: unsupported operating system: $(uname -s)" >&2
        echo "Install with: cargo install aidememo-cli" >&2
        exit 1
        ;;
esac

case "$(uname -m)" in
    arm64|aarch64) arch="aarch64" ;;
    x86_64|amd64) arch="x86_64" ;;
    *)
        echo "error: unsupported architecture: $(uname -m)" >&2
        echo "Install with: cargo install aidememo-cli" >&2
        exit 1
        ;;
esac

if [[ -n "${AIDEMEMO_VERSION:-}" ]]; then
    version="$AIDEMEMO_VERSION"
else
    latest_url="$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest")"
    version="${latest_url##*/}"
fi

if [[ "$version" != v* ]]; then
    echo "error: expected a v-prefixed release, got: $version" >&2
    exit 1
fi

target="$arch-$os"
archive="aidememo-$version-$target.tar.gz"
base_url="https://github.com/$REPO/releases/download/$version"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

echo "→ Downloading AideMemo $version for $target"
curl -fsSL "$base_url/$archive" -o "$tmp_dir/$archive"
curl -fsSL "$base_url/SHA256SUMS" -o "$tmp_dir/SHA256SUMS"

expected="$(awk -v file="$archive" '$2 == file || $2 == "./" file {print $1}' "$tmp_dir/SHA256SUMS")"
if [[ -z "$expected" ]]; then
    echo "error: checksum not found for $archive" >&2
    exit 1
fi

if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$tmp_dir/$archive" | awk '{print $1}')"
else
    require shasum
    actual="$(shasum -a 256 "$tmp_dir/$archive" | awk '{print $1}')"
fi

if [[ "$actual" != "$expected" ]]; then
    echo "error: checksum verification failed for $archive" >&2
    exit 1
fi

tar -xzf "$tmp_dir/$archive" -C "$tmp_dir"
binary="$(find "$tmp_dir" -type f -name aidememo -perm -u+x | head -n 1)"
if [[ -z "$binary" ]]; then
    echo "error: release archive did not contain the aidememo binary" >&2
    exit 1
fi

mkdir -p "$INSTALL_DIR"
install -m 755 "$binary" "$INSTALL_DIR/aidememo"
if ! "$INSTALL_DIR/aidememo" --help >/dev/null; then
    echo "error: installed binary failed its startup check" >&2
    exit 1
fi

echo "✓ Installed AideMemo $version to $INSTALL_DIR/aidememo"
if [[ ":$PATH:" != *":$INSTALL_DIR:"* ]]; then
    echo
    echo "Add AideMemo to your PATH:"
    echo "  export PATH=\"$INSTALL_DIR:\$PATH\""
fi
echo
echo "Next: aidememo init --agent codex ./my-wiki"

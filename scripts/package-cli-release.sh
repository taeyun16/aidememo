#!/usr/bin/env bash

set -euo pipefail

if [[ $# -ne 4 ]]; then
    echo "usage: $0 <version> <target> <binary> <output-dir>" >&2
    exit 2
fi

VERSION="${1#v}"
TARGET="$2"
BINARY="$3"
OUTPUT_DIR="$4"
ARCHIVE_NAME="aidememo-v${VERSION}-${TARGET}"
STAGING_DIR="$OUTPUT_DIR/$ARCHIVE_NAME"

if [[ ! -x "$BINARY" ]]; then
    echo "release binary is missing or not executable: $BINARY" >&2
    exit 1
fi

if ! "$BINARY" --help >/dev/null; then
    echo "release binary failed its --help smoke check: $BINARY" >&2
    exit 1
fi

rm -rf "$STAGING_DIR"
mkdir -p "$STAGING_DIR"
install -m 0755 "$BINARY" "$STAGING_DIR/aidememo"
install -m 0644 README.md LICENSE-MIT LICENSE-APACHE "$STAGING_DIR/"

tar -C "$OUTPUT_DIR" -czf "$OUTPUT_DIR/$ARCHIVE_NAME.tar.gz" "$ARCHIVE_NAME"
rm -rf "$STAGING_DIR"

echo "$OUTPUT_DIR/$ARCHIVE_NAME.tar.gz"

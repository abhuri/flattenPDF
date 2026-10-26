#!/usr/bin/env bash
# Downloads the prebuilt PDFium library (bblanchon/pdfium-binaries) into ./pdfium.
# The version must match the `pdfium_*` feature of pdfium-render in Cargo.toml.
set -euo pipefail

PDFIUM_VERSION="${PDFIUM_VERSION:-7881}"
DEST="${1:-pdfium}"

if [[ -n "${PDFIUM_PLATFORM:-}" ]]; then
  platform="$PDFIUM_PLATFORM"
else
  case "$(uname -s)-$(uname -m)" in
    Linux-x86_64) platform=linux-x64 ;;
    Linux-aarch64) platform=linux-arm64 ;;
    Darwin-*) platform=mac-univ ;;
    MINGW*-x86_64|MSYS*-x86_64|CYGWIN*-x86_64) platform=win-x64 ;;
    MINGW*-aarch64|MSYS*-aarch64) platform=win-arm64 ;;
    *) echo "unsupported platform: $(uname -s)-$(uname -m)" >&2; exit 1 ;;
  esac
fi

url="https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F${PDFIUM_VERSION}/pdfium-${platform}.tgz"
echo "Fetching $url"
mkdir -p "$DEST"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
curl -fsSL "$url" -o "$tmp/pdfium.tgz"
tar -xzf "$tmp/pdfium.tgz" -C "$tmp"

case "$platform" in
  win-*) cp "$tmp/bin/pdfium.dll" "$DEST/" ;;
  mac-*) cp "$tmp/lib/libpdfium.dylib" "$DEST/" ;;
  *) cp "$tmp/lib/libpdfium.so" "$DEST/" ;;
esac
cp "$tmp/LICENSE" "$DEST/PDFIUM_LICENSE" 2>/dev/null || true
echo "PDFium $PDFIUM_VERSION ($platform) installed to $DEST/"

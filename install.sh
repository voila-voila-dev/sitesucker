#!/bin/bash
set -euo pipefail

REPO="voila-dev/sitegrabber"
INSTALL_DIR="/usr/local/bin"
BINARY="sitegrabber"

info() { printf "\033[1;37m%s\033[0m\n" "$*"; }
error() { printf "\033[1;31merror:\033[0m %s\n" "$*" >&2; exit 1; }

# Detect platform
OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
    Linux)  TARGET_OS="linux" ;;
    Darwin) TARGET_OS="darwin" ;;
    *)      error "Unsupported OS: $OS" ;;
esac

case "$ARCH" in
    x86_64|amd64)  TARGET_ARCH="x86_64" ;;
    aarch64|arm64) TARGET_ARCH="aarch64" ;;
    *)             error "Unsupported architecture: $ARCH" ;;
esac

TARGET="${TARGET_OS}-${TARGET_ARCH}"

# Get latest release
info "Fetching latest release..."
LATEST=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name"' | head -1 | cut -d'"' -f4)
[ -z "$LATEST" ] && error "Could not determine latest version"

DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${LATEST}/sitegrabber-${TARGET}.tar.gz"

info "Downloading sitegrabber ${LATEST} for ${TARGET}..."
TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

curl -fsSL "$DOWNLOAD_URL" -o "${TMPDIR}/sitegrabber.tar.gz" || error "Download failed. Check https://github.com/${REPO}/releases"
tar -xzf "${TMPDIR}/sitegrabber.tar.gz" -C "$TMPDIR"

# Install
if [ -w "$INSTALL_DIR" ]; then
    mv "${TMPDIR}/${BINARY}" "${INSTALL_DIR}/${BINARY}"
else
    info "Installing to ${INSTALL_DIR} (requires sudo)..."
    sudo mv "${TMPDIR}/${BINARY}" "${INSTALL_DIR}/${BINARY}"
fi

chmod +x "${INSTALL_DIR}/${BINARY}"

info ""
info "  sitegrabber ${LATEST} installed to ${INSTALL_DIR}/${BINARY}"
info ""
info "  Run 'sitegrabber https://example.com' to get started."
info ""

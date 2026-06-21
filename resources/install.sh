#!/usr/bin/env sh
# mind installer: download the release binary for this platform and drop it on PATH.
#
#   curl --proto '=https' --tlsv1.2 -fsSL https://raw.githubusercontent.com/jaemk/mind/main/resources/install.sh | sh
#
# Honors:
#   MIND_VERSION       version to install (e.g. 0.2.0); default: latest release
#   MIND_INSTALL_DIR   install directory; default: ~/.local/bin
set -eu

REPO="jaemk/mind"
INSTALL_DIR="${MIND_INSTALL_DIR:-$HOME/.local/bin}"

err() {
	echo "mind-install: $*" >&2
	exit 1
}

# A downloader: curl or wget, whichever exists.
fetch() {
	# fetch <url> -> stdout
	if command -v curl >/dev/null 2>&1; then
		curl --proto '=https' --proto-redir '=https' --tlsv1.2 -fsSL "$1"
	elif command -v wget >/dev/null 2>&1; then
		wget --https-only -qO- "$1"
	else
		err "need curl or wget on PATH"
	fi
}

fetch_to() {
	# fetch_to <url> <dest-file>
	if command -v curl >/dev/null 2>&1; then
		curl --proto '=https' --proto-redir '=https' --tlsv1.2 -fsSL "$1" -o "$2"
	else
		wget --https-only -qO "$2" "$1"
	fi
}

# Map uname to a release target triple.
os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
Linux) os_part="unknown-linux-gnu" ;;
Darwin) os_part="apple-darwin" ;;
*) err "unsupported OS: $os (must build from source)" ;;
esac
case "$arch" in
x86_64 | amd64) arch_part="x86_64" ;;
aarch64 | arm64) arch_part="aarch64" ;;
*) err "unsupported architecture: $arch" ;;
esac
target="${arch_part}-${os_part}"

# macOS x86_64 has no prebuilt binary (only Apple Silicon is published).
if [ "$os" = "Darwin" ] && [ "$arch_part" = "x86_64" ]; then
	err "no prebuilt binary for Intel macOS; must build from source"
fi

# Resolve the version: explicit MIND_VERSION, else the latest release tag.
version="${MIND_VERSION:-}"
if [ -z "$version" ]; then
	tag="$(fetch "https://api.github.com/repos/${REPO}/releases/latest" \
		| sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' \
		| head -n 1)"
	[ -n "$tag" ] || err "could not determine the latest release; set MIND_VERSION"
	version="${tag#v}"
fi

asset="mind-${version}-${target}.tar.gz"
url="https://github.com/${REPO}/releases/download/v${version}/${asset}"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "mind-install: downloading ${asset}"
fetch_to "$url" "$tmp/$asset" || err "download failed: $url"
tar -xzf "$tmp/$asset" -C "$tmp" || err "could not extract $asset"
[ -f "$tmp/mind" ] || err "archive did not contain a 'mind' binary"

mkdir -p "$INSTALL_DIR"
install -m 0755 "$tmp/mind" "$INSTALL_DIR/mind" 2>/dev/null \
	|| { cp "$tmp/mind" "$INSTALL_DIR/mind" && chmod 0755 "$INSTALL_DIR/mind"; }

echo "mind-install: installed mind ${version} to ${INSTALL_DIR}/mind"
case ":${PATH}:" in
*":${INSTALL_DIR}:"*) ;;
*) echo "mind-install: add ${INSTALL_DIR} to your PATH, e.g. export PATH=\"${INSTALL_DIR}:\$PATH\"" ;;
esac

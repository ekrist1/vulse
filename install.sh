#!/bin/sh
# Installs the Vulse server binary from GitHub releases.
#
#   curl -fsSL https://raw.githubusercontent.com/ekrist1/vulse/master/install.sh | sh
#
# VULSE_VERSION     a release tag such as v0.2.0 (default: the latest release)
# VULSE_INSTALL_DIR where to put the binary (default: /usr/local/bin)
set -eu

REPO="ekrist1/vulse"
VERSION="${VULSE_VERSION:-latest}"
INSTALL_DIR="${VULSE_INSTALL_DIR:-/usr/local/bin}"

fail() {
    echo "error: $*" >&2
    exit 1
}

[ "$(uname -s)" = "Linux" ] || fail "release binaries are built for Linux only; see the README to build from source or run the Docker image"

case "$(uname -m)" in
    x86_64 | amd64) target="x86_64-unknown-linux-gnu" ;;
    aarch64 | arm64) target="aarch64-unknown-linux-gnu" ;;
    *) fail "unsupported CPU architecture $(uname -m); x86_64 and aarch64 are supported" ;;
esac

# The binaries link against glibc 2.35 or later.
if ldd --version 2>&1 | grep -qi musl; then
    fail "this system uses musl (such as Alpine); use the Docker image instead"
fi

if command -v curl > /dev/null 2>&1; then
    download() { curl -fsSL -o "$2" "$1"; }
elif command -v wget > /dev/null 2>&1; then
    download() { wget -q -O "$2" "$1"; }
else
    fail "curl or wget is required"
fi

if command -v sha256sum > /dev/null 2>&1; then
    verify() { sha256sum -c "$1" > /dev/null; }
elif command -v shasum > /dev/null 2>&1; then
    verify() { shasum -a 256 -c "$1" > /dev/null; }
else
    fail "sha256sum or shasum is required to verify the download"
fi

if [ "$VERSION" = "latest" ]; then
    base="https://github.com/$REPO/releases/latest/download"
else
    case "$VERSION" in v*) ;; *) VERSION="v$VERSION" ;; esac
    base="https://github.com/$REPO/releases/download/$VERSION"
fi

archive="vulse-$target.tar.gz"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM

echo "Downloading $archive ($VERSION)…"
download "$base/$archive" "$tmp/$archive" || fail "could not download $base/$archive"
download "$base/$archive.sha256" "$tmp/$archive.sha256" || fail "could not download the checksum"
(cd "$tmp" && verify "$archive.sha256") || fail "checksum mismatch for $archive"
tar -xzf "$tmp/$archive" -C "$tmp"

if [ -w "$INSTALL_DIR" ] || { [ ! -e "$INSTALL_DIR" ] && mkdir -p "$INSTALL_DIR" 2> /dev/null; }; then
    install -m 755 "$tmp/vulse" "$INSTALL_DIR/vulse"
elif command -v sudo > /dev/null 2>&1; then
    echo "Installing to $INSTALL_DIR with sudo…"
    sudo install -d "$INSTALL_DIR"
    sudo install -m 755 "$tmp/vulse" "$INSTALL_DIR/vulse"
else
    fail "$INSTALL_DIR is not writable; set VULSE_INSTALL_DIR to a directory you own"
fi

echo "Installed $("$INSTALL_DIR/vulse" --version) to $INSTALL_DIR/vulse"
case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *) echo "Add $INSTALL_DIR to your PATH to run it as 'vulse'." ;;
esac
cat << 'EOF'

Start it with `vulse`. It listens on 127.0.0.1:8080 and stores data in the
current directory. On first start it logs a setup token: open /setup in a
browser and enter it to create the first admin.

Running on a server? See https://github.com/ekrist1/vulse#deployment
EOF

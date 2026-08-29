#!/bin/sh
# server-spy universal installer — no sudo needed.
# Installs the prebuilt static binary from GitHub releases:
#   - as root: /usr/local/bin
#   - as a regular user: ~/.local/bin (adds it to PATH for this shell if missing)
# Usage: curl -fsSL https://lennart-rth.github.io/server-spy/install.sh | sh
set -e

REPO="lennart-rth/server-spy"
API="https://api.github.com/repos/$REPO/releases/latest"
BASE="https://github.com/$REPO/releases/download"

case "$(uname -m)" in
    x86_64 | amd64) ARCH="x86_64" ;;
    aarch64 | arm64) ARCH="aarch64" ;;
    *)
        echo "server-spy: unsupported architecture '$(uname -m)'" >&2
        exit 1
        ;;
esac

echo "server-spy: resolving latest release..."
VERSION=$(curl -fsSL "$API" | sed -n 's/.*"tag_name": "v\([^"]*\)".*/\1/p' | head -n1)
if [ -z "$VERSION" ]; then
    echo "server-spy: no prebuilt release found for $REPO — trying to build from source" >&2
    if command -v cargo >/dev/null 2>&1; then
        if [ "$(id -u)" = 0 ]; then
            CARGO_ROOT=/usr/local
        else
            CARGO_ROOT=${SERVER_SPY_BIN_DIR:-$HOME/.local}
        fi
        cargo install --git "https://github.com/$REPO" --locked --root "$CARGO_ROOT" server-spy
        "$CARGO_ROOT/bin/server-spy" --version
        echo "server-spy: installed."
        exit 0
    fi
    echo "server-spy: no prebuilt release and no cargo available." >&2
    echo "  check https://github.com/$REPO/releases" >&2
    exit 1
fi
echo "server-spy: latest version is $VERSION"

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT INT TERM

fetch() {
    target="$1"
    url="$BASE/v$VERSION/server-spy-$VERSION-$target.tar.gz"
    echo "server-spy: downloading $target ..."
    curl -fsSL "$url" | tar -xz -C "$TMP" server-spy
}

# Static musl builds run on any Linux; fall back to glibc if unavailable.
if ! fetch "$ARCH-unknown-linux-musl" 2>/dev/null; then
    echo "server-spy: musl build not found, trying glibc build"
    fetch "$ARCH-unknown-linux-gnu"
fi

if [ "$(id -u)" = 0 ]; then
    DEST=/usr/local/bin
else
    DEST=${SERVER_SPY_BIN_DIR:-$HOME/.local/bin}
fi
mkdir -p "$DEST"
install -m 755 "$TMP/server-spy" "$DEST/server-spy"

case ":$PATH:" in
    *":$DEST:"*) ;;
    *)
        echo "server-spy: installed to $DEST, which is not on your PATH."
        echo "  add it with:  export PATH=\"$DEST:\$PATH\""
        ;;
esac

"$DEST/server-spy" --version
echo "server-spy: installed."

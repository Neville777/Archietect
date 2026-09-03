#!/bin/sh
# Downloads the right prebuilt archietect binary for this machine from the
# latest GitHub Release and installs it onto PATH. No Rust toolchain, no
# clone.
#
#   curl -fsSL https://raw.githubusercontent.com/Neville777/Archietect/main/packaging/install.sh | sh
#   curl -fsSL .../install.sh | sh -s -- --version v0.1.2   # pin a version
#   curl -fsSL .../install.sh | sh -s -- --dir ~/bin        # custom install dir
#
# POSIX sh, deliberately — `curl ... | sh` ignores this file's own shebang
# and runs under whatever `sh` actually is on the machine (often dash, not
# bash). `set -o pipefail` is bash-only and broke this exact invocation on
# a real dash system — caught by actually running the public one-liner, not
# by reading the script. No internal pipes here, so pipefail bought nothing
# anyway; `set -eu` covers everything this script needs.
#
# Windows is deliberately unsupported here — see release.yml's own comment:
# the plain binary has never been built or run on Windows by anyone in this
# project's history.
set -eu

REPO="Neville777/Archietect"
VERSION="latest"
INSTALL_DIR=""

while [ $# -gt 0 ]; do
    case "$1" in
        --version) VERSION="$2"; shift 2 ;;
        --dir) INSTALL_DIR="$2"; shift 2 ;;
        *) echo "unknown argument: $1" >&2; exit 1 ;;
    esac
done

OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
    Linux)
        case "$ARCH" in
            x86_64) ASSET="archietect-linux-x86_64" ;;
            *) echo "archietect: no prebuilt binary for Linux/$ARCH — build from source instead:" >&2
               echo "  git clone https://github.com/$REPO && cd Archietect && cargo build --release" >&2
               exit 1 ;;
        esac
        ;;
    Darwin)
        case "$ARCH" in
            arm64)  ASSET="archietect-macos-arm64" ;;
            x86_64) ASSET="archietect-macos-x86_64" ;;
            *) echo "archietect: no prebuilt binary for macOS/$ARCH — build from source instead." >&2; exit 1 ;;
        esac
        ;;
    *)
        echo "archietect: no prebuilt binary for $OS — this installer supports Linux and macOS only." >&2
        echo "On Windows, or any other platform, build from source: cargo build --release" >&2
        exit 1
        ;;
esac

if [ "$VERSION" = "latest" ]; then
    URL="https://github.com/$REPO/releases/latest/download/$ASSET"
else
    URL="https://github.com/$REPO/releases/download/$VERSION/$ASSET"
fi

if [ -z "$INSTALL_DIR" ]; then
    if [ -w "/usr/local/bin" ] 2>/dev/null; then
        INSTALL_DIR="/usr/local/bin"
    else
        INSTALL_DIR="$HOME/.local/bin"
    fi
fi
mkdir -p "$INSTALL_DIR"

TMP="$(mktemp)"
trap 'rm -f "$TMP"' EXIT

echo "archietect: downloading $ASSET ($VERSION) ..."
if ! curl -fsSL "$URL" -o "$TMP"; then
    echo "archietect: download failed — $URL" >&2
    echo "  either that version doesn't have a release for this platform yet, or the version tag is wrong." >&2
    echo "  check https://github.com/$REPO/releases for what's actually published." >&2
    exit 1
fi

chmod +x "$TMP"
mv "$TMP" "$INSTALL_DIR/archietect"
trap - EXIT

echo "archietect: installed to $INSTALL_DIR/archietect"
case ":$PATH:" in
    *":$INSTALL_DIR:"*) ;;
    *) echo "archietect: $INSTALL_DIR is not on your PATH — add it, e.g.:" >&2
       echo "  export PATH=\"$INSTALL_DIR:\$PATH\"" >&2 ;;
esac

if command -v "$INSTALL_DIR/archietect" >/dev/null 2>&1 || "$INSTALL_DIR/archietect" --version >/dev/null 2>&1; then
    echo "archietect: $("$INSTALL_DIR/archietect" --version 2>/dev/null || echo installed)"
fi

# One-time, global, idempotent: if Claude Code is on this machine, register
# archietect's MCP server automatically — every onboarded project reuses
# this same registration, so it belongs here (install time), not in any
# per-project step. `claude mcp add` exits 1 if already registered (checked
# live: re-adding an existing name errors, doesn't just overwrite), so this
# checks `mcp list` first rather than swallowing that error blindly. Best
# effort only — a failure here never fails the install itself.
if command -v claude >/dev/null 2>&1; then
    if claude mcp list 2>/dev/null | grep -q "^archietect:"; then
        : # already registered, e.g. a prior install or rerun — nothing to do
    elif claude mcp add archietect -- "$INSTALL_DIR/archietect" mcp >/dev/null 2>&1; then
        echo "archietect: registered as an MCP server for Claude Code (every project, automatically)"
    fi
fi

echo
echo "next: cd into a project and run: archietect"
echo "  (first run there indexes it automatically — no separate init step)"

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

# One-time, global, idempotent, best-effort MCP registration — attempted
# for EVERY MCP-speaking client this script has a verified mechanism for,
# not just one. Each client's own real command/config format was checked
# live before being written here (see each block's own comment) rather
# than assumed from another client's shape. A failure in any one of these
# never fails the install itself, and never blocks the others from being
# tried.
ARCHIETECT_MCP_CMD="$INSTALL_DIR/archietect"

# Claude Code — `claude mcp add` exits 1 if already registered (checked
# live: re-adding an existing name errors, doesn't just overwrite), so this
# checks first rather than swallowing that error blindly. Uses `mcp get`,
# NOT `mcp list` — `list` runs a live health check against EVERY configured
# server (including unrelated ones, e.g. Gmail/Drive), so it can stall this
# entire install on network conditions that have nothing to do with
# archietect. Found by actually timing a rerun: `list` hung past two
# minutes in this environment, `get` returns instantly either way.
if command -v claude >/dev/null 2>&1; then
    if claude mcp get archietect >/dev/null 2>&1; then
        : # already registered — nothing to do
    elif claude mcp add archietect -- "$ARCHIETECT_MCP_CMD" mcp >/dev/null 2>&1; then
        echo "archietect: registered as an MCP server for Claude Code (every project, automatically)"
    fi
fi

# Gemini CLI — `gemini mcp add` was checked live too: no `--` separator (its
# arg parser treats everything after the name as a command+args list
# directly, unlike Claude's), and --scope user is required or it silently
# registers project-local only. Unlike Claude, calling `add` again on an
# existing name UPDATES it in place and exits 0 — genuinely idempotent,
# confirmed by actually calling it twice. `gemini mcp list` writes its
# actual output to STDERR, not stdout (confirmed by capturing each stream
# separately) — `2>/dev/null` here would silently discard the very text
# being grepped and make this check always report "not registered."
if command -v gemini >/dev/null 2>&1; then
    if gemini mcp list 2>&1 | grep -q "archietect: $ARCHIETECT_MCP_CMD mcp"; then
        : # already registered with this exact command — nothing to do
    elif gemini mcp add --scope user archietect "$ARCHIETECT_MCP_CMD" mcp >/dev/null 2>&1; then
        echo "archietect: registered as an MCP server for Gemini CLI (every project, automatically)"
    fi
fi

# Cursor — has no CLI for this at all (confirmed against cursor.com/docs/mcp:
# the only documented paths are one-click marketplace install or hand-editing
# ~/.cursor/mcp.json — no `cursor mcp add`). Detected by the presence of
# ~/.cursor/ rather than by running the `cursor` binary, which launches the
# GUI app itself rather than behaving as a scriptable CLI. Written as a
# real JSON merge (jq preferred, python3 fallback — same dual-path already
# used for Claude's settings.json elsewhere in this project), never a
# blind overwrite of a file that may hold a human's other servers.
if [ -d "$HOME/.cursor" ] && ! grep -q "\"archietect\"" "$HOME/.cursor/mcp.json" 2>/dev/null; then
    CURSOR_MCP_JSON="$HOME/.cursor/mcp.json"
    if command -v jq >/dev/null 2>&1; then
        TMP_MCP="$(mktemp)"
        if [ -f "$CURSOR_MCP_JSON" ]; then
            EXISTING="$(cat "$CURSOR_MCP_JSON")"
        else
            EXISTING='{}'
        fi
        if echo "$EXISTING" | jq --arg cmd "$ARCHIETECT_MCP_CMD" \
            '.mcpServers = ((.mcpServers // {}) + {"archietect": {"command": $cmd, "args": ["mcp"]}})' \
            > "$TMP_MCP" 2>/dev/null; then
            mv "$TMP_MCP" "$CURSOR_MCP_JSON"
            echo "archietect: registered as an MCP server for Cursor (every project, automatically)"
        else
            rm -f "$TMP_MCP"
        fi
    elif command -v python3 >/dev/null 2>&1; then
        python3 - "$CURSOR_MCP_JSON" "$ARCHIETECT_MCP_CMD" <<'PYEOF' 2>/dev/null && echo "archietect: registered as an MCP server for Cursor (every project, automatically)"
import json, sys
path, cmd = sys.argv[1], sys.argv[2]
try:
    with open(path) as fh:
        data = json.load(fh)
except (FileNotFoundError, json.JSONDecodeError):
    data = {}
data.setdefault("mcpServers", {})["archietect"] = {"command": cmd, "args": ["mcp"]}
with open(path, "w") as fh:
    json.dump(data, fh, indent=2)
    fh.write("\n")
PYEOF
    fi
fi

echo
echo "archietect: for any other MCP-speaking tool (Codex CLI, Windsurf, ...), the stdio command is:"
echo "  $ARCHIETECT_MCP_CMD mcp"
echo
echo "next: cd into a project and run: archietect"
echo "  (first run there indexes it automatically — no separate init step)"

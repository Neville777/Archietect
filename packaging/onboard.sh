#!/usr/bin/env bash
# The zero-to-first-answer path: clone Architect, run this once per project,
# get a usable state report. Wraps build + `architect init` + MCP
# registration + (opt-in) daemon install. `architect init` remains the real
# CLI entry point for normal use — this script is the convenience layer for
# first-time setup, never a replacement for it.
#
#   packaging/onboard.sh /path/to/project              # prompts for the daemon
#   packaging/onboard.sh /path/to/project --daemon      # force-enable, no prompt
#   packaging/onboard.sh /path/to/project --no-daemon   # force-disable, no prompt
#   packaging/onboard.sh /path/to/project --non-interactive   # CI: never prompt, default no-daemon unless --daemon given
set -euo pipefail

ARCHITECT_REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$ARCHITECT_REPO/target/release/architect"

TARGET=""
DAEMON_FLAG=""       # "", "yes", "no"
NON_INTERACTIVE=0
for arg in "$@"; do
    case "$arg" in
        --daemon) DAEMON_FLAG="yes" ;;
        --no-daemon) DAEMON_FLAG="no" ;;
        --non-interactive) NON_INTERACTIVE=1 ;;
        *) TARGET="$arg" ;;
    esac
done
TARGET="${TARGET:-$PWD}"
mkdir -p "$TARGET"
TARGET="$(cd "$TARGET" && pwd)"

json_get() {
    # $1 = dotted path like ".stats.laws_active", reads JSON from stdin.
    # jq preferred; python3 fallback so the script doesn't hard-depend on a
    # tool that might not be installed.
    if command -v jq >/dev/null 2>&1; then
        jq -r "$1"
    else
        python3 -c '
import sys, json
d = json.load(sys.stdin)
for key in sys.argv[1].strip(".").split("."):
    d = d[key]
print(d)
' "$1" 2>/dev/null || echo "?"
    fi
}

echo "== 1/4  build =="
if [[ -x "$BIN" ]]; then
    echo "   already built: $BIN"
else
    echo "   building (first run only)..."
    (cd "$ARCHITECT_REPO" && cargo build --release)
fi

render_coverage() {
    # $1 = structural_coverage JSON object, read from stdin.
    if command -v jq >/dev/null 2>&1; then
        jq -r '.supported[] | "  " + .language + "  (" + (.files|tostring) + " files) — " + .symbol_support'
        jq -r '.present_but_unsupported[] | "  " + .language + "  (" + (.files|tostring) + " files) — present, no structural extractor"'
    else
        python3 -c '
import sys, json
d = json.load(sys.stdin)
for l in d.get("supported", []):
    print(f"  {l[\"language\"]}  ({l[\"files\"]} files) — {l[\"symbol_support\"]}")
for l in d.get("present_but_unsupported", []):
    print(f"  {l[\"language\"]}  ({l[\"files\"]} files) — present, no structural extractor")
'
    fi
}

echo "== 2/4  index $TARGET =="
INIT_JSON="$("$BIN" init --root "$TARGET")"
FILES=$(echo "$INIT_JSON" | json_get '.files_scanned')
SYMBOLS=$(echo "$INIT_JSON" | json_get '.symbols')
ROUTES=$(echo "$INIT_JSON" | json_get '.routes')
CONCEPTS=$(echo "$INIT_JSON" | json_get '.concepts')
STATUS_JSON="$("$BIN" status --root "$TARGET")"
if command -v jq >/dev/null 2>&1; then
    COVERAGE_JSON="$(echo "$STATUS_JSON" | jq -c '.structural_coverage')"
else
    COVERAGE_JSON="$(echo "$STATUS_JSON" | python3 -c 'import sys,json; print(json.dumps(json.load(sys.stdin)["structural_coverage"]))')"
fi

echo "== 3/4  MCP registration (global — every project reuses this one entry) =="
MCP_OK=0
if command -v claude >/dev/null 2>&1; then
    if claude mcp list 2>/dev/null | grep -q "^architect:"; then
        echo "   already registered: architect -> $BIN mcp"
        MCP_OK=1
    else
        if claude mcp add architect -- "$BIN" mcp >/dev/null 2>&1; then
            echo "   registered: architect -> $BIN mcp"
            MCP_OK=1
        else
            echo "   registration failed — register manually with: claude mcp add architect -- $BIN mcp"
        fi
    fi
else
    echo "   'claude' CLI not found on PATH — skipping; register manually with:"
    echo "     claude mcp add architect -- $BIN mcp"
fi

echo "== 4/4  continuous watching =="
if [[ -z "$DAEMON_FLAG" ]]; then
    if [[ "$NON_INTERACTIVE" -eq 1 ]]; then
        DAEMON_FLAG="no"
        echo "   --non-interactive: defaulting to no daemon (pass --daemon to enable unattended)"
    elif [[ -t 0 && -t 1 ]]; then
        read -r -p "   Enable continuous architecture watching (background daemon)? [y/N] " ans
        case "$ans" in
            [yY]*) DAEMON_FLAG="yes" ;;
            *) DAEMON_FLAG="no" ;;
        esac
    else
        DAEMON_FLAG="no"
        echo "   no TTY to prompt — defaulting to no daemon (pass --daemon to enable)"
    fi
fi

DAEMON_STATE="not installed"
if [[ "$DAEMON_FLAG" == "yes" ]]; then
    case "$(uname)" in
        Linux)
            UNIT_DIR="$HOME/.config/systemd/user"
            mkdir -p "$UNIT_DIR"
            cp "$ARCHITECT_REPO/packaging/architectd.service" "$UNIT_DIR/architectd@.service"
            ESCAPED="$(systemd-escape "$TARGET")"
            systemctl --user daemon-reload
            systemctl --user enable --now "architectd@${ESCAPED}"
            DAEMON_STATE="running"
            echo "   enabled: systemctl --user status architectd@${ESCAPED}"
            ;;
        Darwin)
            # No systemd on macOS — the equivalent is a launchd user
            # LaunchAgent. launchd has no instance-templating like systemd's
            # %I, so generate one fully-substituted plist per watched
            # project, named uniquely by a sanitized copy of its path.
            AGENT_DIR="$HOME/Library/LaunchAgents"
            LOG_DIR="$HOME/Library/Logs/architect"
            mkdir -p "$AGENT_DIR" "$LOG_DIR"
            SANITIZED="$(echo "$TARGET" | sed -e 's|^/||' -e 's|[/ ]|-|g')"
            LABEL="com.architect.watch.${SANITIZED}"
            PLIST="$AGENT_DIR/${LABEL}.plist"
            sed -e "s|__LABEL__|${LABEL}|g" \
                -e "s|__ARCHITECT_BIN__|${BIN}|g" \
                -e "s|__PROJECT_ROOT__|${TARGET}|g" \
                -e "s|__LOG_DIR__|${LOG_DIR}|g" \
                "$ARCHITECT_REPO/packaging/com.architect.watch.plist.template" > "$PLIST"
            # Unload any previous version of this project's agent first so
            # re-onboarding is idempotent, then load via the modern,
            # non-deprecated bootstrap subcommand (launchctl load/unload are
            # deprecated since macOS 10.10; bootstrap/bootout are the
            # replacement, operating on the GUI domain for the current user).
            launchctl bootout "gui/$(id -u)" "$PLIST" >/dev/null 2>&1 || true
            launchctl bootstrap "gui/$(id -u)" "$PLIST"
            launchctl enable "gui/$(id -u)/${LABEL}"
            DAEMON_STATE="running"
            echo "   enabled: launchctl print gui/$(id -u)/${LABEL}"
            echo "   logs: $LOG_DIR/${LABEL}.{out,err}.log"
            ;;
        *)
            echo "   daemon auto-install is not supported on '$(uname)' yet — skipping"
            echo "   (indexing and MCP registration above are unaffected)"
            ;;
    esac
else
    echo "   skipped (rerun with --daemon any time to enable it for $TARGET)"
fi

# ── readiness report ─────────────────────────────────────────────────────────
LAWS_ACTIVE=$("$BIN" laws | json_get '.stats.laws_active')
NAME="$(basename "$TARGET")"
mcp_mark() { [[ "$MCP_OK" -eq 1 ]] && echo "✓" || echo "○"; }
daemon_mark() { [[ "$DAEMON_STATE" == "running" ]] && echo "✓" || echo "○"; }

echo
echo "╭──────────────────────────────────────────╮"
printf "│ %-42s │\n" "           ARCHITECT READY"
echo "╰──────────────────────────────────────────╯"
echo
echo "Project"
printf "  %-10s %s\n" "Name" "$NAME"
printf "  %-10s %s\n" "Root" "$TARGET"
echo
echo "Architecture"
printf "  %-10s %s\n" "Files" "$FILES"
printf "  %-10s %s\n" "Symbols" "$SYMBOLS"
printf "  %-10s %s\n" "Routes" "$ROUTES"
printf "  %-10s %s\n" "Concepts" "$CONCEPTS"
printf "  %-10s %s\n" "Laws" "$LAWS_ACTIVE"
echo
echo "Structural coverage (what Architect can actually see in THIS repo)"
echo "$COVERAGE_JSON" | render_coverage
echo
echo "Integrations"
echo "  ✓ CLI"
echo "  $(mcp_mark) MCP"
echo "  $(daemon_mark) Watch daemon"
echo
echo "Architect is now attached to this project."
echo
echo "Try:"
echo
echo "  architect"
echo "  architect concept <name>"
echo "  architect impact <name>"
echo "  architect duplicates"
echo "  architect owner <term>"
echo "  architect history"
echo
if [[ "$MCP_OK" -eq 1 ]]; then
    echo "For AI tools: MCP registration is already configured."
fi

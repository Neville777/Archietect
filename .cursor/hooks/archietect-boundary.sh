#!/bin/bash
# archietect:managed — installed by packaging/onboard.sh --cursor-hook.
# The Cursor adapter for archietect's permission boundary. Pure
# translation, zero policy — see onboard.sh's own comment above this
# heredoc for the confirmed Cursor hook schema this relies on.
ARCHIETECT_BIN="/home/nevo/Personal_Projects/archietect/target/release/archietect"
INPUT="$(cat)"
command -v jq >/dev/null 2>&1 || { echo '{"permission":"allow"}'; exit 0; }
FILE_PATH="$(echo "$INPUT" | jq -r '.tool_input.file_path // empty')"
if [ -z "$FILE_PATH" ]; then
    echo '{"permission":"allow"}'
    exit 0
fi

BIN_TO_USE="$ARCHIETECT_BIN"
[ -x "$BIN_TO_USE" ] || BIN_TO_USE="archietect"
if ! command -v "$BIN_TO_USE" >/dev/null 2>&1 && [ ! -x "$BIN_TO_USE" ]; then
    echo '{"permission":"allow"}'
    exit 0
fi

# Project root comes from the hook payload itself (Cursor's `cwd` field) —
# there is no confirmed Cursor equivalent of Claude Code's
# CLAUDE_PROJECT_DIR env var, so none is assumed.
PROJECT_ROOT="$(echo "$INPUT" | jq -r '.cwd // empty')"
[ -n "$PROJECT_ROOT" ] || PROJECT_ROOT="$PWD"

DECISION_JSON="$("$BIN_TO_USE" permissions-check --path "$FILE_PATH" --domain code --root "$PROJECT_ROOT" 2>/dev/null)"
if [ -z "$DECISION_JSON" ]; then
    echo '{"permission":"allow"}'
    exit 0
fi
# NOT `.allowed // empty` — jq's `//` treats JSON `false` as falsy and would
# silently turn a real denial into the empty string here, exactly the bug
# found and fixed in the Claude Code adapter this was copied from.
ALLOWED="$(echo "$DECISION_JSON" | jq -r 'if has("allowed") then (.allowed | tostring) else "true" end' 2>/dev/null)"
REASON="$(echo "$DECISION_JSON" | jq -r '.reason // empty' 2>/dev/null)"

if [ "$ALLOWED" = "false" ]; then
    # jq's \(...) string interpolation, not bash-quoted concatenation — the
    # earlier version built the JSON via '...'"'"' + $fp + '"'"'...' string
    # gluing and silently produced "access to  +  + " on every real denial,
    # because $REASON itself legitimately contains single quotes (e.g.
    # "path contains '.ssh'") that collided with the bash quoting. Found by
    # actually running this against a real .ssh path, not by reading the
    # jq expression and assuming it worked.
    jq -n --arg fp "$FILE_PATH" --arg reason "$REASON" '
        {
            permission: "deny",
            user_message: "archietect: access to \($fp) is denied by the permission boundary — \($reason).",
            agent_message: "This path is blocked by the archietect permission boundary (\($reason)). This is not a request; choose a different path."
        }
    '
    exit 0
fi

echo '{"permission":"allow"}'
exit 0

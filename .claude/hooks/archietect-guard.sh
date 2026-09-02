#!/bin/bash
# archietect:managed — installed by packaging/onboard.sh --claude-hook.
# PreToolUse hook on Write: if the path Claude is about to write does NOT
# already exist (i.e. this is genuinely a new file, not a rewrite of one
# that's already there), check whether the basename resolves to a concept
# archietect already knows about. Fails open on anything ambiguous — this
# guards against confident duplicates, not a general-purpose nuisance.
ARCHIETECT_BIN="/home/nevo/Personal_Projects/archietect/target/release/archietect"
INPUT="$(cat)"
command -v jq >/dev/null 2>&1 || exit 0
FILE_PATH="$(echo "$INPUT" | jq -r '.tool_input.file_path // empty')"
[ -n "$FILE_PATH" ] || exit 0
[ -e "$FILE_PATH" ] && exit 0   # rewriting something that already exists — not "creating new"

NAME="$(basename "$FILE_PATH")"
NAME="${NAME%.*}"
[ -n "$NAME" ] || exit 0

PROJECT_ROOT="${CLAUDE_PROJECT_DIR:-$PWD}"
BIN_TO_USE="$ARCHIETECT_BIN"
[ -x "$BIN_TO_USE" ] || BIN_TO_USE="archietect"
command -v "$BIN_TO_USE" >/dev/null 2>&1 || [ -x "$BIN_TO_USE" ] || exit 0
[ -f "$PROJECT_ROOT/archietect.db" ] || exit 0

VERDICT_JSON="$("$BIN_TO_USE" concept "$NAME" --root "$PROJECT_ROOT" 2>/dev/null)" || exit 0
VERDICT="$(echo "$VERDICT_JSON" | jq -r '.verdict // empty' 2>/dev/null)"
case "$VERDICT" in
    ""|ABSENT|INSUFFICIENT_COVERAGE) exit 0 ;;
    *)
        echo "archietect: '$NAME' already resolves to a $VERDICT concept in this codebase — run \`archietect concept $NAME\` to see the evidence before creating $FILE_PATH. If this really is a new, unrelated thing, proceed." >&2
        exit 2
        ;;
esac

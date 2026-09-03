#!/usr/bin/env bash
# The zero-to-first-answer path: clone Archietect, run this once per project,
# get a usable state report. Wraps build + `archietect init` + MCP
# registration + (opt-in) daemon install. `archietect init` remains the real
# CLI entry point for normal use — this script is the convenience layer for
# first-time setup, never a replacement for it.
#
#   packaging/onboard.sh /path/to/project              # prompts for the daemon, git hook, and Claude Code hook
#   packaging/onboard.sh /path/to/project --daemon      # force-enable, no prompt
#   packaging/onboard.sh /path/to/project --no-daemon   # force-disable, no prompt
#   packaging/onboard.sh /path/to/project --git-hook    # force-enable the pre-commit gate, no prompt
#   packaging/onboard.sh /path/to/project --no-git-hook
#   packaging/onboard.sh /path/to/project --claude-hook    # force-enable the Claude Code PreToolUse gate, no prompt
#   packaging/onboard.sh /path/to/project --no-claude-hook
#   packaging/onboard.sh /path/to/project --cursor-hook    # force-enable the Cursor preToolUse gate, no prompt
#   packaging/onboard.sh /path/to/project --no-cursor-hook
#   packaging/onboard.sh /path/to/project --non-interactive   # CI: never prompt, default "no" for all adapters unless the matching --flag is given
#
# --claude-hook and --cursor-hook are separate, independent adapters into the
# SAME vendor-neutral boundary (permissions::check_resource, exposed as
# `archietect permissions-check`) — see SYSTEM_MEMORY.md's "Instruction
# files vs. memory vs. enforcement". Installing one implies nothing about
# the other; a Cursor-only project never touches .claude/, and vice versa.
set -euo pipefail

ARCHIETECT_REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$ARCHIETECT_REPO/target/release/archietect"

TARGET=""
DAEMON_FLAG=""       # "", "yes", "no"
GIT_HOOK_FLAG=""     # "", "yes", "no"
CLAUDE_HOOK_FLAG=""  # "", "yes", "no"
CURSOR_HOOK_FLAG=""  # "", "yes", "no"
NON_INTERACTIVE=0
for arg in "$@"; do
    case "$arg" in
        --daemon) DAEMON_FLAG="yes" ;;
        --no-daemon) DAEMON_FLAG="no" ;;
        --git-hook) GIT_HOOK_FLAG="yes" ;;
        --no-git-hook) GIT_HOOK_FLAG="no" ;;
        --claude-hook) CLAUDE_HOOK_FLAG="yes" ;;
        --no-claude-hook) CLAUDE_HOOK_FLAG="no" ;;
        --cursor-hook) CURSOR_HOOK_FLAG="yes" ;;
        --no-cursor-hook) CURSOR_HOOK_FLAG="no" ;;
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

echo "== 1/8  build =="
if [[ -x "$BIN" ]]; then
    echo "   already built: $BIN"
else
    echo "   building (first run only)..."
    (cd "$ARCHIETECT_REPO" && cargo build --release)
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

echo "== 2/8  index $TARGET =="
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

echo "== 3/8  MCP registration (global — every project reuses this one entry) =="
MCP_OK=0
if command -v claude >/dev/null 2>&1; then
    if claude mcp list 2>/dev/null | grep -q "^archietect:"; then
        echo "   already registered: archietect -> $BIN mcp"
        MCP_OK=1
    else
        if claude mcp add archietect -- "$BIN" mcp >/dev/null 2>&1; then
            echo "   registered: archietect -> $BIN mcp"
            MCP_OK=1
        else
            echo "   registration failed — register manually with: claude mcp add archietect -- $BIN mcp"
        fi
    fi
else
    echo "   'claude' CLI not found on PATH — skipping; register manually with:"
    echo "     claude mcp add archietect -- $BIN mcp"
fi

echo "== 4/8  agent instructions =="
# Registering the MCP server makes archietect *available*; it doesn't make
# an agent reach for it instead of grepping/reading the tree by hand — an
# agent that isn't told to check first, won't. Non-destructive: append-only,
# marked so a re-run never duplicates the block, and an existing AGENTS.md
# is never overwritten past that one marked section.
#
# AGENTS.md only, deliberately — not also a CLAUDE.md. The content is
# generic (no Claude-specific wording at all), and AGENTS.md is already the
# vendor-neutral convention several tools read directly (including Codex).
# Writing a second, identically-worded CLAUDE.md file gave a Claude-only
# impression to a project that in fact has nothing Claude-specific in it —
# found by a direct question about exactly that. The Claude-SPECIFIC piece
# of this script is the separate, explicitly opt-in --claude-hook flag
# below, which is honestly named for what it is.
AGENT_INSTRUCTIONS='<!-- archietect:agent-instructions:begin -->
## Before you create something new

This project has [archietect](https://github.com/Neville777/Archietect)
attached — a deterministic index of what already exists here (concepts,
schema, routes, decisions), kept warm by a background daemon.

Before creating a new table, model, endpoint, or class — or before telling
someone something does not exist in this codebase — check first:

    archietect concept <name>       # does it exist? canonical? evidence?
    archietect intent "<goal>"      # smallest correct change: EXTEND vs CREATE
    archietect impact <name>        # what breaks if you change it
    archietect duplicates           # suspected redundant concepts already here

If archietect is registered as an MCP server in this environment, call
these as tools instead of shelling out. A commit that introduces a
duplicate concept may also be rejected automatically by a pre-commit hook —
see `archietect ci` if that happens.
<!-- archietect:agent-instructions:end -->'

write_agent_instructions() {
    local f="$1"
    if [[ -f "$f" ]] && grep -q "archietect:agent-instructions:begin" "$f"; then
        echo "   already present: $f"
        return
    fi
    if [[ -f "$f" ]]; then
        printf '\n%s\n' "$AGENT_INSTRUCTIONS" >> "$f"
    else
        printf '%s\n' "$AGENT_INSTRUCTIONS" > "$f"
    fi
    echo "   written: $f"
}
write_agent_instructions "$TARGET/AGENTS.md"

echo "== 5/8  commit gate (pre-commit hook) =="
# The instructions above are advisory — an agent that doesn't feel like
# checking, won't. This is the actual gate: it inspects the STAGED DIFF, so
# it rejects a violation regardless of whether a human, Claude, Cursor, or
# Copilot wrote it. `archietect ci` already exists for exactly this
# (src/main.rs's Cmd::Ci: exits 1 on a real violation, 0 otherwise).
if [[ -z "$GIT_HOOK_FLAG" ]]; then
    if [[ "$NON_INTERACTIVE" -eq 1 ]]; then
        GIT_HOOK_FLAG="no"
        echo "   --non-interactive: defaulting to no commit gate (pass --git-hook to enable unattended)"
    elif [[ -t 0 && -t 1 ]]; then
        read -r -p "   Install a pre-commit hook that blocks architectural violations? [y/N] " ans
        case "$ans" in
            [yY]*) GIT_HOOK_FLAG="yes" ;;
            *) GIT_HOOK_FLAG="no" ;;
        esac
    else
        GIT_HOOK_FLAG="no"
        echo "   no TTY to prompt — defaulting to no commit gate (pass --git-hook to enable)"
    fi
fi

GIT_HOOK_STATE="not installed"
if [[ "$GIT_HOOK_FLAG" == "yes" ]]; then
    if [[ ! -d "$TARGET/.git" ]]; then
        echo "   not a git repository — skipping (no .git in $TARGET)"
    else
        HOOK_DIR="$TARGET/.git/hooks"
        HOOK_FILE="$HOOK_DIR/pre-commit"
        mkdir -p "$HOOK_DIR"
        if [[ -f "$HOOK_FILE" ]] && ! grep -q "archietect:managed" "$HOOK_FILE" 2>/dev/null; then
            echo "   an existing pre-commit hook is already at $HOOK_FILE — not overwriting it"
            echo "   add this line to it yourself if you want the gate:"
            echo "     git diff --cached | \"$BIN\" ci"
        else
            sed -e "s|__ARCHIETECT_BIN__|${BIN}|g" > "$HOOK_FILE" <<'HOOK'
#!/bin/sh
# archietect:managed — installed by packaging/onboard.sh --git-hook.
# Rejects a commit whose staged diff violates an architectural law (e.g. a
# CREATE TABLE duplicating an existing concept). Runs on the diff itself, so
# it works regardless of whether a human, Claude, Cursor, or Copilot wrote
# it — no reliance on the author having consulted archietect first.
ARCHIETECT_BIN="__ARCHIETECT_BIN__"
REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null)"
if [ -n "$REPO_ROOT" ] && [ -f "$REPO_ROOT/archietect.db" ]; then
    if [ -x "$ARCHIETECT_BIN" ]; then
        git diff --cached | "$ARCHIETECT_BIN" ci
        exit $?
    elif command -v archietect >/dev/null 2>&1; then
        git diff --cached | archietect ci
        exit $?
    fi
fi
exit 0
HOOK
            chmod +x "$HOOK_FILE"
            GIT_HOOK_STATE="installed"
            echo "   installed: $HOOK_FILE"
        fi
    fi
else
    echo "   skipped (rerun with --git-hook any time to enable it for $TARGET)"
fi

echo "== 6/8  Claude Code edit gate =="
# The commit gate above catches a violation at commit time — this catches it
# earlier, before the write even lands, but ONLY inside Claude Code and ONLY
# for genuinely NEW files (an existing file being legitimately rewritten via
# Write is never blocked — the guard script checks archietect concept for a
# NEW path's basename, not every Write). Scoped to the `Write` tool only:
# `Edit` targets a file that already exists, so "is this creating something
# new" isn't a question Edit's old_string/new_string diff can answer cheaply
# or reliably — false positives there would be pure friction.
if [[ -z "$CLAUDE_HOOK_FLAG" ]]; then
    if [[ "$NON_INTERACTIVE" -eq 1 ]]; then
        CLAUDE_HOOK_FLAG="no"
        echo "   --non-interactive: defaulting to no Claude Code hook (pass --claude-hook to enable unattended)"
    elif [[ -t 0 && -t 1 ]]; then
        read -r -p "   Install a Claude Code hook that blocks creating a file for an already-known concept? [y/N] " ans
        case "$ans" in
            [yY]*) CLAUDE_HOOK_FLAG="yes" ;;
            *) CLAUDE_HOOK_FLAG="no" ;;
        esac
    else
        CLAUDE_HOOK_FLAG="no"
        echo "   no TTY to prompt — defaulting to no Claude Code hook (pass --claude-hook to enable)"
    fi
fi

CLAUDE_HOOK_STATE="not installed"
if [[ "$CLAUDE_HOOK_FLAG" == "yes" ]]; then
    HOOKS_DIR="$TARGET/.claude/hooks"
    GUARD_SCRIPT="$HOOKS_DIR/archietect-guard.sh"
    BOUNDARY_SCRIPT="$HOOKS_DIR/archietect-boundary.sh"
    SESSION_START_SCRIPT="$HOOKS_DIR/archietect-session-start.sh"
    SETTINGS_FILE="$TARGET/.claude/settings.json"
    GUARD_CMD='$CLAUDE_PROJECT_DIR/.claude/hooks/archietect-guard.sh'
    BOUNDARY_CMD='$CLAUDE_PROJECT_DIR/.claude/hooks/archietect-boundary.sh'
    SESSION_START_CMD='$CLAUDE_PROJECT_DIR/.claude/hooks/archietect-session-start.sh'
    mkdir -p "$HOOKS_DIR"

    sed -e "s|__ARCHIETECT_BIN__|${BIN}|g" > "$GUARD_SCRIPT" <<'GUARD'
#!/bin/bash
# archietect:managed — installed by packaging/onboard.sh --claude-hook.
# PreToolUse hook on Write: if the path Claude is about to write does NOT
# already exist (i.e. this is genuinely a new file, not a rewrite of one
# that's already there), check whether the basename resolves to a concept
# archietect already knows about. Fails open on anything ambiguous — this
# guards against confident duplicates, not a general-purpose nuisance.
ARCHIETECT_BIN="__ARCHIETECT_BIN__"
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
GUARD
    chmod +x "$GUARD_SCRIPT"

    # The second hook: the Claude Code ADAPTER for archietect's permission
    # boundary — not the boundary itself. The boundary
    # (permissions::check_resource, src/permissions.rs) is vendor-neutral
    # and already reachable by any tool via CLI/REST/MCP; this script's only
    # job is translating Claude Code's specific pre-action moment
    # (a PreToolUse hook) into one `archietect permissions-check` call and
    # translating its {allowed, reason} back into Claude Code's specific
    # blocking mechanism (stderr + exit 2). No denial logic is reimplemented
    # here — an adapter is plumbing, not policy, so a Cursor/Codex/future
    # adapter is a new translation of the same call, not a re-design. See
    # SYSTEM_MEMORY.md's "Instruction files vs. memory vs. enforcement" for
    # the full one-bag-many-adapters shape this fits into. CLAUDE.md can be
    # forgotten, misread, or skipped; this cannot, because it runs before
    # Claude's Read/Edit/Write is allowed to land, and a denial here is not
    # a request. Fails open on anything ambiguous (missing jq/binary, no
    # archietect.db, a permissions-check error) — this hook exists to
    # enforce an ESTABLISHED denial, never to become a general nuisance that
    # blocks on uncertainty. Bash's own arbitrary shell commands are outside
    # this hook's reach on purpose: a path buried inside a shell pipeline
    # isn't reliably parseable, so this covers Read/Edit/Write only and does
    # not claim to cover Bash.
    sed -e "s|__ARCHIETECT_BIN__|${BIN}|g" > "$BOUNDARY_SCRIPT" <<'BOUNDARY'
#!/bin/bash
# archietect:managed — the Claude Code ADAPTER for archietect's permission
# boundary (see onboard.sh's comment above this heredoc, and
# SYSTEM_MEMORY.md's "Instruction files vs. memory vs. enforcement"). This
# script owns zero policy: it only translates a PreToolUse call into
# `archietect permissions-check` and translates the JSON answer into Claude
# Code's own block signal (stderr + exit 2). Any other tool's adapter
# should look like this shape, translated to that tool's own mechanism.
ARCHIETECT_BIN="__ARCHIETECT_BIN__"
INPUT="$(cat)"
command -v jq >/dev/null 2>&1 || exit 0
FILE_PATH="$(echo "$INPUT" | jq -r '.tool_input.file_path // empty')"
[ -n "$FILE_PATH" ] || exit 0

PROJECT_ROOT="${CLAUDE_PROJECT_DIR:-$PWD}"
BIN_TO_USE="$ARCHIETECT_BIN"
[ -x "$BIN_TO_USE" ] || BIN_TO_USE="archietect"
command -v "$BIN_TO_USE" >/dev/null 2>&1 || [ -x "$BIN_TO_USE" ] || exit 0

DECISION_JSON="$("$BIN_TO_USE" permissions-check --path "$FILE_PATH" --domain code --root "$PROJECT_ROOT" 2>/dev/null)" || exit 0
# NOT `.allowed // empty` — jq's `//` treats JSON `false` as falsy, so a
# real denial (allowed: false) silently became the empty string here and
# this hook fail-opened on every single denial it existed to catch. Found
# live by actually running the hook against a real .ssh path, not by
# reading the jq expression and assuming it did what it looked like it did.
ALLOWED="$(echo "$DECISION_JSON" | jq -r 'if has("allowed") then (.allowed | tostring) else "true" end' 2>/dev/null)"
REASON="$(echo "$DECISION_JSON" | jq -r '.reason // empty' 2>/dev/null)"
[ "$ALLOWED" = "false" ] || exit 0

echo "archietect: access to '$FILE_PATH' is denied by the permission boundary — $REASON. This is not a request; the operation is rejected." >&2
exit 2
BOUNDARY
    chmod +x "$BOUNDARY_SCRIPT"

    # The third hook: register-first, not remember-to-read. See
    # SYSTEM_MEMORY.md's "Instruction files vs. memory vs. enforcement" —
    # an instruction file like AGENTS.md only works if the agent reads it;
    # this hands the register to the model automatically at the start of
    # every session/resume/clear/compact, so asking the memory is the
    # default instead of a habit that has to be remembered. Uses SessionStart
    # specifically because it's one of the few Claude Code hook events where
    # PLAIN stdout (no JSON wrapper) becomes context the model can see and
    # act on — confirmed directly against the current official docs
    # (code.claude.com/docs/en/hooks: "The exceptions are UserPromptSubmit,
    # UserPromptExpansion, SessionStart, and PostModelSwitch, where Claude
    # Code adds plain-text stdout as context") — not the more complex
    # hookSpecificOutput.additionalContext JSON form some other sources
    # describe, which has open, unresolved reliability issues reported
    # against it. Honest limit, stated here because it matters: this script
    # can be verified to run correctly and print the right content; whether
    # Claude Code's runtime actually surfaces that stdout into a live
    # session's context is an external behavior no shell command can
    # mechanically confirm from inside this repository.
    sed -e "s|__ARCHIETECT_BIN__|${BIN}|g" > "$SESSION_START_SCRIPT" <<'SESSIONSTART'
#!/bin/bash
# archietect:managed — installed by packaging/onboard.sh --claude-hook.
# SessionStart hook: prints `archietect register --compact` to stdout so a
# new/resumed/cleared/compacted session sees, up front, what this memory
# already knows/doesn't know/allows — without the model having to remember
# to ask. Fails silent (empty stdout, exit 0) on anything ambiguous: this
# is a convenience, never a thing that should make a session fail to start.
ARCHIETECT_BIN="__ARCHIETECT_BIN__"
cat >/dev/null  # SessionStart's stdin isn't needed for this hook — read and discard it cleanly rather than leaving it unconsumed.

PROJECT_ROOT="${CLAUDE_PROJECT_DIR:-$PWD}"
BIN_TO_USE="$ARCHIETECT_BIN"
[ -x "$BIN_TO_USE" ] || BIN_TO_USE="archietect"
command -v "$BIN_TO_USE" >/dev/null 2>&1 || [ -x "$BIN_TO_USE" ] || exit 0
[ -f "$PROJECT_ROOT/archietect.db" ] || exit 0

REGISTER_JSON="$("$BIN_TO_USE" register --root "$PROJECT_ROOT" --compact 2>/dev/null)" || exit 0
[ -n "$REGISTER_JSON" ] || exit 0

echo "archietect: this project's memory (run \`archietect register\` any time for the live view) —"
echo "$REGISTER_JSON"
exit 0
SESSIONSTART
    chmod +x "$SESSION_START_SCRIPT"

    if [[ -f "$SETTINGS_FILE" ]] && grep -q "archietect-guard.sh" "$SETTINGS_FILE" 2>/dev/null && grep -q "archietect-boundary.sh" "$SETTINGS_FILE" 2>/dev/null && grep -q "archietect-session-start.sh" "$SETTINGS_FILE" 2>/dev/null; then
        echo "   already present: $SETTINGS_FILE"
        CLAUDE_HOOK_STATE="installed"
    elif [[ -f "$SETTINGS_FILE" ]]; then
        if command -v jq >/dev/null 2>&1; then
            TMP_SETTINGS="$(mktemp)"
            jq --arg guard "$GUARD_CMD" --arg boundary "$BOUNDARY_CMD" --arg sessionstart "$SESSION_START_CMD" '
                .hooks.PreToolUse = (
                    (.hooks.PreToolUse // [])
                    + (if any(.hooks.PreToolUse[]?; .hooks[]?.command == $guard) then [] else
                        [{"matcher": "Write", "hooks": [{"type": "command", "command": $guard}]}] end)
                    + (if any(.hooks.PreToolUse[]?; .hooks[]?.command == $boundary) then [] else
                        [{"matcher": "Read|Edit|Write", "hooks": [{"type": "command", "command": $boundary}]}] end)
                )
                | .hooks.SessionStart = (
                    (.hooks.SessionStart // [])
                    + (if any(.hooks.SessionStart[]?; .hooks[]?.command == $sessionstart) then [] else
                        [{"matcher": "startup|resume|clear|compact|fork", "hooks": [{"type": "command", "command": $sessionstart}]}] end)
                )
            ' "$SETTINGS_FILE" > "$TMP_SETTINGS" && mv "$TMP_SETTINGS" "$SETTINGS_FILE"
            echo "   updated: $SETTINGS_FILE"
            CLAUDE_HOOK_STATE="installed"
        elif command -v python3 >/dev/null 2>&1; then
            python3 - "$SETTINGS_FILE" "$GUARD_CMD" "$BOUNDARY_CMD" "$SESSION_START_CMD" <<'PYEOF'
import json, sys
path, guard_cmd, boundary_cmd, session_start_cmd = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
with open(path) as fh:
    data = json.load(fh)
pre = data.setdefault("hooks", {}).setdefault("PreToolUse", [])
existing_cmds = {h.get("command") for entry in pre for h in entry.get("hooks", [])}
if guard_cmd not in existing_cmds:
    pre.append({"matcher": "Write", "hooks": [{"type": "command", "command": guard_cmd}]})
if boundary_cmd not in existing_cmds:
    pre.append({"matcher": "Read|Edit|Write", "hooks": [{"type": "command", "command": boundary_cmd}]})
start = data["hooks"].setdefault("SessionStart", [])
start_cmds = {h.get("command") for entry in start for h in entry.get("hooks", [])}
if session_start_cmd not in start_cmds:
    start.append({"matcher": "startup|resume|clear|compact|fork", "hooks": [{"type": "command", "command": session_start_cmd}]})
with open(path, "w") as fh:
    json.dump(data, fh, indent=2)
    fh.write("\n")
PYEOF
            echo "   updated: $SETTINGS_FILE"
            CLAUDE_HOOK_STATE="installed"
        else
            echo "   $SETTINGS_FILE already exists and neither jq nor python3 is available to merge safely — skipping."
            echo "   add these manually to its \"hooks\" object:"
            echo "     \"PreToolUse\": [{\"matcher\": \"Write\", \"hooks\": [{\"type\": \"command\", \"command\": \"$GUARD_CMD\"}]},"
            echo "                    {\"matcher\": \"Read|Edit|Write\", \"hooks\": [{\"type\": \"command\", \"command\": \"$BOUNDARY_CMD\"}]}]"
            echo "     \"SessionStart\": [{\"matcher\": \"startup|resume|clear|compact|fork\", \"hooks\": [{\"type\": \"command\", \"command\": \"$SESSION_START_CMD\"}]}]"
        fi
    else
        cat > "$SETTINGS_FILE" <<EOF
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Write",
        "hooks": [
          {
            "type": "command",
            "command": "$GUARD_CMD"
          }
        ]
      },
      {
        "matcher": "Read|Edit|Write",
        "hooks": [
          {
            "type": "command",
            "command": "$BOUNDARY_CMD"
          }
        ]
      }
    ],
    "SessionStart": [
      {
        "matcher": "startup|resume|clear|compact|fork",
        "hooks": [
          {
            "type": "command",
            "command": "$SESSION_START_CMD"
          }
        ]
      }
    ]
  }
}
EOF
        echo "   written: $SETTINGS_FILE"
        CLAUDE_HOOK_STATE="installed"
    fi
else
    echo "   skipped (rerun with --claude-hook any time to enable it for $TARGET)"
fi

echo "== 7/8  Cursor edit gate =="
# The Cursor ADAPTER for the same vendor-neutral boundary the Claude Code
# adapter above uses — see SYSTEM_MEMORY.md's "Instruction files vs. memory
# vs. enforcement" ("one boundary, many thin adapters"). This script owns
# zero policy: it only translates Cursor's preToolUse hook (confirmed
# against https://cursor.com/docs/hooks before writing this — Cursor's own
# schema, not assumed from Claude Code's) into one `archietect
# permissions-check` call, and translates {allowed, reason} into Cursor's
# own response shape ({"permission": "allow"|"deny", ...}), never into
# Claude Code's (stderr + exit 2). Confirmed schema: hooks.json's
# `preToolUse` entries take a `matcher` string of pipe-separated tool types
# — Cursor's documented values are Shell/Read/Write/Grep/Delete/Task/
# MCP:<name>, with NO separate "Edit" type the way Claude Code has, so
# "Read|Write" is the closest available match to the Claude adapter's
# "Read|Edit|Write" scope, not a narrower choice made on purpose. stdin
# carries `tool_name`/`tool_input.file_path`/`cwd` (project root arrives in
# the payload itself, unlike Claude Code's CLAUDE_PROJECT_DIR env var — no
# equivalent env var was found or assumed). stdout must be
# `{"permission": "allow"|"deny", "user_message": ..., "agent_message":
# ...}` — exit 2 alone is documented as deny-equivalent but doesn't carry a
# reason, so this script always prints the full JSON and exits 0, matching
# Cursor's own documented example script rather than mixing mechanisms.
if [[ -z "$CURSOR_HOOK_FLAG" ]]; then
    if [[ "$NON_INTERACTIVE" -eq 1 ]]; then
        CURSOR_HOOK_FLAG="no"
        echo "   --non-interactive: defaulting to no Cursor hook (pass --cursor-hook to enable unattended)"
    elif [[ -t 0 && -t 1 ]]; then
        read -r -p "   Install a Cursor hook that enforces the archietect permission boundary? [y/N] " ans
        case "$ans" in
            [yY]*) CURSOR_HOOK_FLAG="yes" ;;
            *) CURSOR_HOOK_FLAG="no" ;;
        esac
    else
        CURSOR_HOOK_FLAG="no"
        echo "   no TTY to prompt — defaulting to no Cursor hook (pass --cursor-hook to enable)"
    fi
fi

CURSOR_HOOK_STATE="not installed"
if [[ "$CURSOR_HOOK_FLAG" == "yes" ]]; then
    CURSOR_HOOKS_DIR="$TARGET/.cursor/hooks"
    CURSOR_BOUNDARY_SCRIPT="$CURSOR_HOOKS_DIR/archietect-boundary.sh"
    CURSOR_CONFIG_FILE="$TARGET/.cursor/hooks.json"
    CURSOR_BOUNDARY_CMD=".cursor/hooks/archietect-boundary.sh"
    mkdir -p "$CURSOR_HOOKS_DIR"

    sed -e "s|__ARCHIETECT_BIN__|${BIN}|g" > "$CURSOR_BOUNDARY_SCRIPT" <<'CURSORBOUNDARY'
#!/bin/bash
# archietect:managed — installed by packaging/onboard.sh --cursor-hook.
# The Cursor adapter for archietect's permission boundary. Pure
# translation, zero policy — see onboard.sh's own comment above this
# heredoc for the confirmed Cursor hook schema this relies on.
ARCHIETECT_BIN="__ARCHIETECT_BIN__"
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
CURSORBOUNDARY
    chmod +x "$CURSOR_BOUNDARY_SCRIPT"

    if [[ -f "$CURSOR_CONFIG_FILE" ]] && grep -q "archietect-boundary.sh" "$CURSOR_CONFIG_FILE" 2>/dev/null; then
        echo "   already present: $CURSOR_CONFIG_FILE"
        CURSOR_HOOK_STATE="installed"
    elif [[ -f "$CURSOR_CONFIG_FILE" ]]; then
        if command -v jq >/dev/null 2>&1; then
            TMP_CURSOR_CONFIG="$(mktemp)"
            jq --arg cmd "$CURSOR_BOUNDARY_CMD" '
                .version = (.version // 1) |
                .hooks.preToolUse = (
                    (.hooks.preToolUse // [])
                    + (if any(.hooks.preToolUse[]?; .command == $cmd) then [] else
                        [{"command": $cmd, "matcher": "Read|Write"}] end)
                )
            ' "$CURSOR_CONFIG_FILE" > "$TMP_CURSOR_CONFIG" && mv "$TMP_CURSOR_CONFIG" "$CURSOR_CONFIG_FILE"
            echo "   updated: $CURSOR_CONFIG_FILE"
            CURSOR_HOOK_STATE="installed"
        elif command -v python3 >/dev/null 2>&1; then
            python3 - "$CURSOR_CONFIG_FILE" "$CURSOR_BOUNDARY_CMD" <<'PYEOF'
import json, sys
path, cmd = sys.argv[1], sys.argv[2]
with open(path) as fh:
    data = json.load(fh)
data.setdefault("version", 1)
pre = data.setdefault("hooks", {}).setdefault("preToolUse", [])
if cmd not in {h.get("command") for h in pre}:
    pre.append({"command": cmd, "matcher": "Read|Write"})
with open(path, "w") as fh:
    json.dump(data, fh, indent=2)
    fh.write("\n")
PYEOF
            echo "   updated: $CURSOR_CONFIG_FILE"
            CURSOR_HOOK_STATE="installed"
        else
            echo "   $CURSOR_CONFIG_FILE already exists and neither jq nor python3 is available to merge safely — skipping."
            echo "   add this manually to its \"hooks\".\"preToolUse\" array:"
            echo "     {\"command\": \"$CURSOR_BOUNDARY_CMD\", \"matcher\": \"Read|Write\"}"
        fi
    else
        cat > "$CURSOR_CONFIG_FILE" <<EOF
{
  "version": 1,
  "hooks": {
    "preToolUse": [
      {
        "command": "$CURSOR_BOUNDARY_CMD",
        "matcher": "Read|Write"
      }
    ]
  }
}
EOF
        echo "   written: $CURSOR_CONFIG_FILE"
        CURSOR_HOOK_STATE="installed"
    fi
else
    echo "   skipped (rerun with --cursor-hook any time to enable it for $TARGET)"
fi

echo "== 8/8  continuous watching =="
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
            cp "$ARCHIETECT_REPO/packaging/archietectd.service" "$UNIT_DIR/archietectd@.service"
            ESCAPED="$(systemd-escape "$TARGET")"
            systemctl --user daemon-reload
            systemctl --user enable --now "archietectd@${ESCAPED}"
            DAEMON_STATE="running"
            echo "   enabled: systemctl --user status archietectd@${ESCAPED}"
            ;;
        Darwin)
            # No systemd on macOS — the equivalent is a launchd user
            # LaunchAgent. launchd has no instance-templating like systemd's
            # %I, so generate one fully-substituted plist per watched
            # project, named uniquely by a sanitized copy of its path.
            AGENT_DIR="$HOME/Library/LaunchAgents"
            LOG_DIR="$HOME/Library/Logs/archietect"
            mkdir -p "$AGENT_DIR" "$LOG_DIR"
            SANITIZED="$(echo "$TARGET" | sed -e 's|^/||' -e 's|[/ ]|-|g')"
            LABEL="com.archietect.watch.${SANITIZED}"
            PLIST="$AGENT_DIR/${LABEL}.plist"
            sed -e "s|__LABEL__|${LABEL}|g" \
                -e "s|__ARCHIETECT_BIN__|${BIN}|g" \
                -e "s|__PROJECT_ROOT__|${TARGET}|g" \
                -e "s|__LOG_DIR__|${LOG_DIR}|g" \
                "$ARCHIETECT_REPO/packaging/com.archietect.watch.plist.template" > "$PLIST"
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
git_hook_mark() { [[ "$GIT_HOOK_STATE" == "installed" ]] && echo "✓" || echo "○"; }
claude_hook_mark() { [[ "$CLAUDE_HOOK_STATE" == "installed" ]] && echo "✓" || echo "○"; }
cursor_hook_mark() { [[ "$CURSOR_HOOK_STATE" == "installed" ]] && echo "✓" || echo "○"; }

echo
echo "╭──────────────────────────────────────────╮"
printf "│ %-42s │\n" "           ARCHIETECT READY"
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
echo "Structural coverage (what Archietect can actually see in THIS repo)"
echo "$COVERAGE_JSON" | render_coverage
echo
echo "Integrations"
echo "  ✓ CLI"
echo "  $(mcp_mark) MCP"
echo "  ✓ Agent instructions (AGENTS.md)"
echo "  $(git_hook_mark) Commit gate (pre-commit hook)"
echo "  $(claude_hook_mark) Claude Code edit gate"
echo "  $(cursor_hook_mark) Cursor edit gate"
echo "  $(daemon_mark) Watch daemon"
echo
echo "Archietect is now attached to this project."
echo
echo "Try:"
echo
echo "  archietect"
echo "  archietect concept <name>"
echo "  archietect impact <name>"
echo "  archietect duplicates"
echo "  archietect owner <term>"
echo "  archietect history"
echo
if [[ "$MCP_OK" -eq 1 ]]; then
    echo "For AI tools: MCP registration is already configured."
fi

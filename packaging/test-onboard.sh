#!/usr/bin/env bash
# Automated regression for packaging/onboard.sh — the "clean machine" test
# matrix, codified instead of run by hand in a terminal each time. Exercises
# the actual onboard.sh script end to end against real scratch state; does
# not touch any real project, systemd unit outside a throwaway path, or the
# global MCP registration beyond what onboard.sh itself already does
# idempotently.
#
#   packaging/test-onboard.sh
set -euo pipefail

ARCHIETECT_REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$ARCHIETECT_REPO/target/release/archietect"
ONBOARD="$ARCHIETECT_REPO/packaging/onboard.sh"

pass=0
fail=0
check() {
    if eval "$2"; then
        echo "  ✓ $1"
        pass=$((pass + 1))
    else
        echo "  ✗ $1"
        fail=$((fail + 1))
    fi
}

echo "== building (if needed) =="
[[ -x "$BIN" ]] || (cd "$ARCHIETECT_REPO" && cargo build --release)

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
PROJECT="$TMP/project"
mkdir -p "$PROJECT"
cat > "$PROJECT/schema.prisma" <<'EOF'
model Widget {
  id   Int    @id @default(autoincrement())
  name String
}
EOF

echo
echo "== Test 1: fresh project =="
git -C "$PROJECT" init -q
git -C "$PROJECT" config user.email test@test.com
git -C "$PROJECT" config user.name test
git -C "$PROJECT" add schema.prisma
git -C "$PROJECT" commit -q -m init
"$ONBOARD" "$PROJECT" --non-interactive --git-hook --claude-hook --cursor-hook > "$TMP/run1.log" 2>&1
check "binary exists"                 "[[ -x \"$BIN\" ]]"
check "project database created"      "[[ -f \"$PROJECT/archietect.db\" ]]"
check "archietect status works"        "\"$BIN\" status --root \"$PROJECT\" >/dev/null"

# Confirmed real and intermittent (~1-in-200, needs the FULL onboard.sh
# flow to manifest — a tight, isolated `init` then `concept` loop run 300
# times back to back never reproduced it, ruling out a simple race in
# that specific transition alone). Bare pass/fail told us nothing on the
# one occurrence caught so far, so on failure this captures everything
# that could matter — real diagnostic capacity for the next time this
# actually fires, rather than reporting UNFIXED as FIXED.
CONCEPT_OUT="$("$BIN" concept --root "$PROJECT" Widget 2>&1)"
if echo "$CONCEPT_OUT" | grep -q '"canonical": "Widget"'; then
    echo "  ✓ archietect concept works"
    pass=$((pass + 1))
else
    echo "  ✗ archietect concept works"
    fail=$((fail + 1))
    mkdir -p /tmp/archietect-test-onboard-diagnostics
    DIAG="/tmp/archietect-test-onboard-diagnostics/concept-flake-$(date +%s%N).log"
    {
        echo "=== archietect concept works — FAILURE DIAGNOSTIC ==="
        echo "--- concept output ---"
        echo "$CONCEPT_OUT"
        echo "--- archietect.db stat ---"
        stat "$PROJECT/archietect.db" 2>&1
        echo "--- schema.prisma stat ---"
        stat "$PROJECT/schema.prisma" 2>&1
        echo "--- onboard.sh's own run1.log ---"
        cat "$TMP/run1.log"
        echo "--- immediate retry (does it self-heal?) ---"
        "$BIN" concept --root "$PROJECT" Widget 2>&1
        echo "--- full project directory listing ---"
        ls -la "$PROJECT"
    } > "$DIAG" 2>&1
    echo "    diagnostic captured: $DIAG"
fi

check "readiness report printed"      "grep -q 'ARCHIETECT READY' \"$TMP/run1.log\""

echo
echo "== Test 1b: agent instructions written =="
check "AGENTS.md written"                  "grep -q 'archietect:agent-instructions:begin' \"$PROJECT/AGENTS.md\""
check "no CLAUDE.md written (AGENTS.md only — see onboard.sh's own comment)" "[[ ! -f \"$PROJECT/CLAUDE.md\" ]]"

echo
echo "== Test 1c: commit gate actually blocks a real duplicate =="
echo "CREATE TABLE Widgets (id INT PRIMARY KEY, name TEXT);" > "$PROJECT/dup.sql"
git -C "$PROJECT" add dup.sql
DUP_COMMIT_RC=0
git -C "$PROJECT" commit -q -m "add duplicate widgets table" >/dev/null 2>&1 || DUP_COMMIT_RC=$?
check "pre-commit hook rejected the duplicate" "[[ \"$DUP_COMMIT_RC\" -ne 0 ]]"
check "duplicate commit never landed"          "! git -C \"$PROJECT\" log --oneline | grep -q 'duplicate widgets table'"
git -C "$PROJECT" reset -q
rm -f "$PROJECT/dup.sql"
echo "console.log(1)" > "$PROJECT/unrelated.js"
git -C "$PROJECT" add unrelated.js
git -C "$PROJECT" commit -q -m "add unrelated file"
check "pre-commit hook allows a non-duplicate change" "git -C \"$PROJECT\" log --oneline | grep -q 'add unrelated file'"

echo
echo "== Test 1d: Claude Code guard script blocks a duplicate, allows the rest =="
GUARD="$PROJECT/.claude/hooks/archietect-guard.sh"
check "guard script installed"        "[[ -x \"$GUARD\" ]]"
check "settings.json references it"   "grep -q archietect-guard.sh \"$PROJECT/.claude/settings.json\""
BLOCK_RC=0
CLAUDE_PROJECT_DIR="$PROJECT" bash -c "echo '{\"tool_input\": {\"file_path\": \"$PROJECT/Widget.ts\"}}' | \"$GUARD\"" >/dev/null 2>&1 || BLOCK_RC=$?
check "guard blocks a new file matching an existing concept" "[[ \"$BLOCK_RC\" -eq 2 ]]"
ALLOW_RC=0
CLAUDE_PROJECT_DIR="$PROJECT" bash -c "echo '{\"tool_input\": {\"file_path\": \"$PROJECT/TotallyNewThing.ts\"}}' | \"$GUARD\"" >/dev/null 2>&1 || ALLOW_RC=$?
check "guard allows a genuinely new name" "[[ \"$ALLOW_RC\" -eq 0 ]]"
touch "$PROJECT/Widget.ts"
REWRITE_RC=0
CLAUDE_PROJECT_DIR="$PROJECT" bash -c "echo '{\"tool_input\": {\"file_path\": \"$PROJECT/Widget.ts\"}}' | \"$GUARD\"" >/dev/null 2>&1 || REWRITE_RC=$?
check "guard allows rewriting a file that already exists" "[[ \"$REWRITE_RC\" -eq 0 ]]"
rm -f "$PROJECT/Widget.ts"

echo
echo "== Test 1e: Claude Code boundary hook enforces the permission boundary =="
BOUNDARY="$PROJECT/.claude/hooks/archietect-boundary.sh"
check "boundary script installed"      "[[ -x \"$BOUNDARY\" ]]"
check "settings.json references it"    "grep -q archietect-boundary.sh \"$PROJECT/.claude/settings.json\""
DENY_RC=0
CLAUDE_PROJECT_DIR="$PROJECT" bash -c "echo '{\"tool_input\": {\"file_path\": \"$PROJECT/.ssh/id_rsa\"}}' | \"$BOUNDARY\"" >/dev/null 2>&1 || DENY_RC=$?
check "boundary blocks a .ssh path (exit 2)" "[[ \"$DENY_RC\" -eq 2 ]]"
DENY2_RC=0
CLAUDE_PROJECT_DIR="$PROJECT" bash -c "echo '{\"tool_input\": {\"file_path\": \"$PROJECT/config/secrets.json\"}}' | \"$BOUNDARY\"" >/dev/null 2>&1 || DENY2_RC=$?
check "boundary blocks a credential-shaped filename (exit 2)" "[[ \"$DENY2_RC\" -eq 2 ]]"
ALLOW2_RC=0
CLAUDE_PROJECT_DIR="$PROJECT" bash -c "echo '{\"tool_input\": {\"file_path\": \"$PROJECT/src/main.rs\"}}' | \"$BOUNDARY\"" >/dev/null 2>&1 || ALLOW2_RC=$?
check "boundary allows a plain source path (exit 0)" "[[ \"$ALLOW2_RC\" -eq 0 ]]"

echo
echo "== Test 1f: Cursor boundary hook enforces the same permission boundary =="
CURSOR_BOUNDARY="$PROJECT/.cursor/hooks/archietect-boundary.sh"
check "cursor boundary script installed"   "[[ -x \"$CURSOR_BOUNDARY\" ]]"
check "hooks.json references it"           "grep -q archietect-boundary.sh \"$PROJECT/.cursor/hooks.json\""
# Written to temp files rather than bash variables: the real denial JSON
# legitimately contains single quotes (permissions.rs's reason format uses
# 'path contains '.ssh''), which breaks re-quoting a captured value back
# into another single-quoted shell string — found by hitting exactly that
# bug while writing this test, not a hypothetical.
CURSOR_DENY_FILE="$TMP/cursor-deny.json"
echo "{\"tool_name\":\"Read\",\"tool_input\":{\"file_path\":\"$PROJECT/.ssh/id_rsa\"},\"cwd\":\"$PROJECT\"}" | "$CURSOR_BOUNDARY" > "$CURSOR_DENY_FILE"
check "cursor boundary denies a .ssh path"              "[[ \$(jq -r .permission \"$CURSOR_DENY_FILE\") == 'deny' ]]"
check "cursor boundary denial names the real reason"    "jq -r .user_message \"$CURSOR_DENY_FILE\" | grep -qF \"'.ssh'\""
CURSOR_DENY2_FILE="$TMP/cursor-deny2.json"
echo "{\"tool_name\":\"Write\",\"tool_input\":{\"file_path\":\"$PROJECT/config/secrets.json\"},\"cwd\":\"$PROJECT\"}" | "$CURSOR_BOUNDARY" > "$CURSOR_DENY2_FILE"
check "cursor boundary denies a credential-shaped filename" "[[ \$(jq -r .permission \"$CURSOR_DENY2_FILE\") == 'deny' ]]"
CURSOR_ALLOW_FILE="$TMP/cursor-allow.json"
echo "{\"tool_name\":\"Write\",\"tool_input\":{\"file_path\":\"$PROJECT/src/main.rs\"},\"cwd\":\"$PROJECT\"}" | "$CURSOR_BOUNDARY" > "$CURSOR_ALLOW_FILE"
check "cursor boundary allows a plain source path"      "[[ \$(jq -r .permission \"$CURSOR_ALLOW_FILE\") == 'allow' ]]"
check "claude settings.json does not reference the cursor script" "! grep -q '.cursor/hooks' \"$PROJECT/.claude/settings.json\""
check "cursor hooks.json does not reference the claude script"    "! grep -q '.claude/hooks' \"$PROJECT/.cursor/hooks.json\""

echo
echo "== Test 1g: Claude Code SessionStart hook injects the register view =="
SESSION_START="$PROJECT/.claude/hooks/archietect-session-start.sh"
check "session-start script installed"    "[[ -x \"$SESSION_START\" ]]"
check "settings.json references it"       "grep -q archietect-session-start.sh \"$PROJECT/.claude/settings.json\""
check "settings.json wires it to SessionStart, not PreToolUse" "python3 -c \"import json; d=json.load(open('$PROJECT/.claude/settings.json')); ss=d['hooks']['SessionStart']; assert any('archietect-session-start.sh' in h['command'] for e in ss for h in e['hooks'])\""
SESSION_START_OUT="$TMP/session-start.out"
CLAUDE_PROJECT_DIR="$PROJECT" bash -c "echo '{}' | \"$SESSION_START\"" > "$SESSION_START_OUT" 2>/dev/null
check "session-start hook exits 0"                 "CLAUDE_PROJECT_DIR=\"$PROJECT\" bash -c \"echo '{}' | \\\"$SESSION_START\\\"\" >/dev/null 2>&1"
check "session-start hook prints the register view" "grep -q 'this project'\\''s memory' \"$SESSION_START_OUT\""
check "session-start output embeds valid JSON"       "tail -n +2 \"$SESSION_START_OUT\" | jq -e . >/dev/null"
check "session-start output names a known domain"    "tail -n +2 \"$SESSION_START_OUT\" | jq -e '.boundary.domains[] | select(.domain==\"git\")' >/dev/null"
NO_DB_OUT="$TMP/session-start-nodb.out"
NO_DB_PROJECT="$TMP/no-db-project"
mkdir -p "$NO_DB_PROJECT"
CLAUDE_PROJECT_DIR="$NO_DB_PROJECT" bash -c "echo '{}' | \"$SESSION_START\"" > "$NO_DB_OUT" 2>/dev/null
check "session-start is silent (no db) rather than erroring" "[[ ! -s \"$NO_DB_OUT\" ]]"

echo
echo "== Test 2: existing architecture memory survives re-onboarding =="
cat > "$PROJECT/archietect.toml" <<'EOF'
[aliases]
gadget = "Widget"

[[decision]]
id = "widget-is-canonical"
decision = "Widgets are canonical, not Gadget"
because = "test fixture"
rejected = ["separate Gadget table"]
links = ["Widget"]
EOF
echo "" | "$BIN" ci --root "$PROJECT" >/dev/null 2>&1 || true
BEFORE_SIZE=$(stat -c%s "$PROJECT/archietect.db")
"$ONBOARD" "$PROJECT" --non-interactive > "$TMP/run2.log" 2>&1
AFTER_SIZE=$(stat -c%s "$PROJECT/archietect.db")
check "alias still resolves after re-onboarding" "\"$BIN\" concept --root \"$PROJECT\" gadget | grep -q '\"canonical\": \"Widget\"'"
check "decision text untouched"                  "grep -q 'widget-is-canonical' \"$PROJECT/archietect.toml\""
check "history event survived"                   "\"$BIN\" history --root \"$PROJECT\" --limit 5 | grep -q ci_passed"
check "db did not shrink (no data loss)"         "[[ $AFTER_SIZE -ge $BEFORE_SIZE ]]"

echo
echo "== Test 3: rerun is idempotent =="
"$ONBOARD" "$PROJECT" --non-interactive > "$TMP/run3.log" 2>&1
"$ONBOARD" "$PROJECT" --non-interactive > "$TMP/run4.log" 2>&1
check "no duplicate MCP registration lines" "[[ \$(grep -c 'already registered\\|registered:' \"$TMP/run4.log\") -le 1 ]]"
check "concept still resolves after 4 runs" "\"$BIN\" concept --root \"$PROJECT\" Widget | grep -q '\"canonical\": \"Widget\"'"

echo
echo "== Test 4: coverage report reflects real language content =="
check "structural_coverage present"     "\"$BIN\" status --root \"$PROJECT\" | grep -q structural_coverage"

DAEMON_TESTED=0
if command -v systemctl >/dev/null 2>&1 && systemctl --user status >/dev/null 2>&1; then
    echo
    echo "== Test 5: daemon actually watches (systemd --user available) =="
    "$ONBOARD" "$PROJECT" --daemon > "$TMP/run5.log" 2>&1
    ESCAPED="$(systemd-escape "$PROJECT")"
    check "daemon unit active" "systemctl --user is-active --quiet \"archietectd@${ESCAPED}\""
    cat > "$PROJECT/schema2.prisma" <<'EOF'
model Sprocket {
  id Int @id @default(autoincrement())
}
EOF
    sleep 3
    check "daemon observed the new file live" "\"$BIN\" concept --root \"$PROJECT\" Sprocket | grep -q DECLARED_ONLY"
    systemctl --user disable --now "archietectd@${ESCAPED}" >/dev/null 2>&1 || true
    DAEMON_TESTED=1
else
    echo
    echo "== Test 5: skipped (no systemd --user session available in this environment) =="
fi

echo
echo "== Results: $pass passed, $fail failed =="
[[ "$DAEMON_TESTED" -eq 0 ]] && echo "(daemon test skipped, not failed — no systemd --user session here)"
[[ "$fail" -eq 0 ]]

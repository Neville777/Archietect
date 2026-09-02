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
"$ONBOARD" "$PROJECT" --non-interactive --git-hook --claude-hook > "$TMP/run1.log" 2>&1
check "binary exists"                 "[[ -x \"$BIN\" ]]"
check "project database created"      "[[ -f \"$PROJECT/archietect.db\" ]]"
check "archietect status works"        "\"$BIN\" status --root \"$PROJECT\" >/dev/null"
check "archietect concept works"       "\"$BIN\" concept --root \"$PROJECT\" Widget | grep -q '\"canonical\": \"Widget\"'"
check "readiness report printed"      "grep -q 'ARCHIETECT READY' \"$TMP/run1.log\""

echo
echo "== Test 1b: agent instructions written =="
check "AGENTS.md written"                  "grep -q 'archietect:agent-instructions:begin' \"$PROJECT/AGENTS.md\""
check "CLAUDE.md written"                  "grep -q 'archietect:agent-instructions:begin' \"$PROJECT/CLAUDE.md\""

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

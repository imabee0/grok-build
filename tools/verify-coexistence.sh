#!/usr/bin/env bash
# bcode must not disturb any other agent CLI's state.
#
# Every surviving upstream identifier is a coexistence bug -- a path, env var or
# socket bcode would share with another install -- so this test is the
# behavioural half of the brand gate. It runs against a fixture HOME so it never
# touches the developer's real directories.
set -uo pipefail
cd "$(dirname "$0")/.."
REPO=$PWD
BIN=${BCODE_BIN:-$REPO/target/debug/bcode}

if [ ! -x "$BIN" ]; then
  echo "coexistence: no binary at $BIN (build first, or set BCODE_BIN)" >&2
  exit 1
fi

FIXTURE=$(mktemp -d)
trap 'rm -rf "$FIXTURE"' EXIT
HOMEDIR=$FIXTURE/home
WORK=$FIXTURE/work
mkdir -p "$HOMEDIR" "$WORK"

# Populate the homes of the CLIs bcode must leave alone, including the files
# upstream is known to read: settings, permissions, rules, skills, agents.
for tool in .grok .claude .cursor .codex; do
  mkdir -p "$HOMEDIR/$tool/rules" "$HOMEDIR/$tool/skills" "$HOMEDIR/$tool/agents"
  echo '{"permissions":{"defaultMode":"bypassPermissions"},"env":{"SENTINEL":"1"}}' \
    > "$HOMEDIR/$tool/settings.json"
  echo "# $tool rules"        > "$HOMEDIR/$tool/rules/base.md"
  echo '{"sentinel":true}'    > "$HOMEDIR/$tool/auth.json"
  echo "# $tool instructions" > "$HOMEDIR/$tool/AGENTS.md"
done
mkdir -p "$WORK/.claude" "$WORK/.cursor/rules" "$WORK/.grok"
echo '{"permissions":{"allow":["Bash"]}}' > "$WORK/.claude/settings.json"
echo "# cursor rule"                     > "$WORK/.cursor/rules/r.md"
echo "# project"                         > "$WORK/AGENTS.md"
git -C "$WORK" init -q 2>/dev/null || true

manifest() { find "$1" -type f -exec sha256sum {} + 2>/dev/null | sed "s|$1||" | sort; }

before=$(manifest "$HOMEDIR")
before_work=$(manifest "$WORK")

# A non-interactive turn is enough to exercise config load, discovery, skills,
# plugins and auth lookup -- every path that reads another tool's files. It is
# expected to fail for lack of credentials; the state it touches is the point.
( cd "$WORK" && env -i HOME="$HOMEDIR" PATH="$PATH" TERM=dumb \
    "$BIN" -p "say hi" >"$FIXTURE/out.log" 2>&1 )
echo "coexistence: binary exited $? (a credential failure here is fine)"

after=$(manifest "$HOMEDIR")
after_work=$(manifest "$WORK")

status=0
if [ "$before" != "$after" ]; then
  echo "coexistence: FAIL - another tool's home changed:"
  diff <(echo "$before") <(echo "$after") | head -30
  status=1
fi
if [ "$before_work" != "$after_work" ]; then
  echo "coexistence: FAIL - project-local vendor config changed:"
  diff <(echo "$before_work") <(echo "$after_work") | head -30
  status=1
fi

# bcode may create its own home and nothing else.
created=$(find "$HOMEDIR" -mindepth 1 -maxdepth 1 -newer "$HOMEDIR/.grok" 2>/dev/null \
          | grep -v '/\.bcode$' || true)
stray=$(find "$HOMEDIR" -mindepth 1 -maxdepth 1 -type d \
        ! -name '.grok' ! -name '.claude' ! -name '.cursor' ! -name '.codex' \
        ! -name '.bcode' || true)
if [ -n "$stray" ]; then
  echo "coexistence: FAIL - unexpected directories created in HOME:"
  echo "$stray"
  status=1
fi

[ "$status" -eq 0 ] && echo "coexistence: clean - other CLI homes byte-identical"
exit "$status"

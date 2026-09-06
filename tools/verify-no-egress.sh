#!/usr/bin/env bash
# The egress guarantee: bcode makes no network call the user's chosen provider
# did not cause.
#
# Behavioural, not a grep. A fixture HOME with no credentials and no provider
# selected is given a session; every connect(2) the process attempts is
# recorded. Loopback is allowed (the leader socket, the status-line command,
# a local model server); anything else is a call home and fails the test.
#
# Usage: tools/verify-no-egress.sh [path/to/bcode]
set -uo pipefail
cd "$(dirname "$0")/.."

BIN="${1:-target/debug/bcode}"
if [ ! -x "$BIN" ]; then
  echo "egress gate: SKIP - no binary at $BIN (run: cargo build -p bcode-pager-bin)"
  exit 0
fi
if ! command -v strace >/dev/null; then
  echo "egress gate: SKIP - strace not installed"
  exit 0
fi

FIXTURE=$(mktemp -d)
trap 'rm -rf "$FIXTURE"' EXIT
TRACE="$FIXTURE/connect.log"

mkdir -p "$FIXTURE/home"

# No API key in the environment, and a home of its own: nothing here selects a
# provider, so a correct run reaches the network zero times. Two runs, because
# they take different paths: `--version` never opens a session, while a headless
# prompt runs the whole startup (config, catalog, session spawn) before it fails
# for want of a key.
run() {
  env -i \
    HOME="$FIXTURE/home" \
    PATH="/usr/bin:/bin" \
    TERM=dumb \
    strace -f -qq -e trace=connect -o "$1" \
    "$PWD/$BIN" "${@:2}" >/dev/null 2>&1
}

run "$TRACE.version" --version
run "$TRACE.prompt" -p "hello"
cat "$TRACE.version" "$TRACE.prompt" > "$TRACE"

# AF_INET/AF_INET6 only: AF_UNIX connects are local sockets, not egress.
# 127.0.0.0/8 and ::1 are loopback and equally local.
offenders=$(grep -E 'connect\(.*(AF_INET|AF_INET6)' "$TRACE" \
  | grep -vE 'inet_addr\("127\.|inet6_addr\("::1"' || true)

if [ -n "$offenders" ]; then
  echo "egress gate: FAIL - the binary reached the network with no provider selected:"
  echo "$offenders" | head -20
  exit 1
fi

echo "egress gate: clean (no non-loopback connect with no provider selected)"

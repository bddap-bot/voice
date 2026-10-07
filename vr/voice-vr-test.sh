#!/bin/sh
# shellcheck disable=SC2016
set -eu
supervisor=$1
work=$(mktemp -d); trap 'rm -rf "$work"' EXIT
mkdir "$work/bin" "$work/stub"
cp "$supervisor" "$work/bin/voice-vr"
chmod +x "$work/bin/voice-vr"
printf '#!/bin/sh\nexit 0\n' > "$work/stub/pgrep"
chmod +x "$work/stub/pgrep"
host() {
  { echo '#!/bin/sh'; echo "n=\$(cat '$work/runs' 2>/dev/null || echo 0); echo \$((n + 1)) > '$work/runs'"; echo "$1"; } > "$work/bin/voice-vr-host"
  chmod +x "$work/bin/voice-vr-host"
  rm -f "$work/runs"
}
run() { PATH="$work/stub:$PATH" "$work/bin/voice-vr" 2>/dev/null && rc=0 || rc=$?; runs=$(cat "$work/runs"); }
fail() { echo "voice-vr supervisor: $*"; exit 1; }

host 'exit 0'; run
[ "$rc" = 0 ] && [ "$runs" = 1 ] || fail "a quit from SteamVR: rc $rc after $runs runs"

host 'kill -TERM $$'; run
[ "$rc" = 143 ] && [ "$runs" = 1 ] || fail "a deliberate stop: rc $rc after $runs runs"

host '[ "$n" -ge 2 ] && exit 0; [ "$n" = 0 ] && exit 101; kill -ABRT $$'; run
[ "$rc" = 0 ] && [ "$runs" = 3 ] || fail "a panic, then an abort, then a quit: rc $rc after $runs runs"

host 'exit 101'; run
[ "$rc" = 101 ] && [ "$runs" = 5 ] || fail "a crash loop: rc $rc after $runs runs"

printf '#!/bin/sh\nexit 1\n' > "$work/stub/pgrep"
host 'exit 101'; run
[ "$rc" = 101 ] && [ "$runs" = 1 ] || fail "a crash with vrserver gone: rc $rc after $runs runs"

printf '#!/bin/sh\nexit 0\n' > "$work/stub/pgrep"
host 'echo $$ > "$0.pid"; exec sleep 30'
PATH="$work/stub:$PATH" "$work/bin/voice-vr" 2>/dev/null & supervisor_pid=$!
while [ ! -e "$work/runs" ]; do sleep 0.1; done
kill -TERM "$supervisor_pid"
wait "$supervisor_pid" && rc=0 || rc=$?
[ "$rc" = 143 ] || fail "stopping the supervisor: rc $rc"
! kill -0 "$(cat "$work/bin/voice-vr-host.pid")" 2>/dev/null || fail "the host outlived its supervisor"
echo "voice-vr supervisor: 6 checks passed"

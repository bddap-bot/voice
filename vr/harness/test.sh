set -euo pipefail
vrh=$1
stubs=$(mktemp -d); trap 'rm -rf "$stubs"' EXIT
me=$(id -u); other=$((me + 1))
cat > "$stubs/ps" <<S
#!/bin/sh
n=\$(cat "$stubs/ps-calls" 2>/dev/null || echo 0); echo \$((n + 1)) > "$stubs/ps-calls"
[ -e "$stubs/ps-fails" ] && exit 1
cat "$stubs/ps-out"
[ -e "$stubs/foreign-after" ] && [ "\$n" -ge "\$(cat "$stubs/foreign-after")" ] && echo "$other vrserver"
exit 0
S
cat > "$stubs/pasta" <<S
#!/bin/sh
printf '%s\n' "\$*" > "$stubs/pasta-args"
exec sleep "\${PASTA_SECONDS:-0}"
S
chmod +x "$stubs/ps" "$stubs/pasta"
mkdir "$stubs/vrh"
run() { (cd "$stubs" && PATH="$stubs:$PATH" VRH="$stubs/vrh" "$vrh" -- true) && rc=0 || rc=$?; }
reset() { rm -f "$stubs/pasta-args" "$stubs/ps-calls" "$stubs/ps-fails" "$stubs/foreign-after"; printf '%s\n' "1 systemd" "$me vrserver" > "$stubs/ps-out"; }

reset; run
[ "$rc" = 0 ] || { echo "own vrserver: rc $rc"; exit 1; }
case " $(cat "$stubs/pasta-args") " in *" -t none -u none -T none -U none --no-map-gw -- bwrap "*) ;; *) echo "pasta args: $(cat "$stubs/pasta-args")"; exit 1 ;; esac

reset; echo "$other vrserver" >> "$stubs/ps-out"; run
[ "$rc" = 75 ] && [ ! -e "$stubs/pasta-args" ] || { echo "foreign vrserver: rc $rc, harness started: $([ -e "$stubs/pasta-args" ] && echo yes || echo no)"; exit 1; }

reset; touch "$stubs/ps-fails"; run
[ "$rc" = 75 ] && [ ! -e "$stubs/pasta-args" ] || { echo "unlistable processes: rc $rc"; exit 1; }

reset; echo 2 > "$stubs/foreign-after"; start=$(date +%s); PASTA_SECONDS=60 run
[ "$rc" = 75 ] && [ $(($(date +%s) - start)) -lt 30 ] || { echo "vrserver started mid-run: rc $rc after $(($(date +%s) - start)) s"; exit 1; }
echo "vrh: 4 launcher checks passed"

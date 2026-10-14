#!/bin/sh
# SteamVR launches the overlay once and never again, and the gesture and wake word live inside it, so a crash would leave nothing to summon it with.
here=$(dirname "$(readlink -f "$0")")
export VOICE_VR_SPEAKER_MODEL="${VOICE_VR_SPEAKER_MODEL:-$here/../share/speaker.onnx}"
export VOICE_VR_ACTION_MODEL="${VOICE_VR_ACTION_MODEL:-$here/../share/action}"
export VOICE_VR_TURN_MODEL="${VOICE_VR_TURN_MODEL:-$here/../share/turn.onnx}"
environment=${XDG_CONFIG_HOME:-$HOME/.config}/voice-vr/environment
if [ -r "$environment" ]; then
  set -a
  # shellcheck disable=SC1090
  . "$environment"
  set +a
fi
crashes=${XDG_STATE_HOME:-$HOME/.local/state}/voice-vr/crashes
quick=0
child=
trap 'if [ -n "$child" ]; then kill "$child" 2>/dev/null; wait "$child"; fi; exit 143' TERM INT HUP
while :; do
  started=$(date +%s)
  "$here/voice-vr-host" "$@" &
  child=$!
  wait "$child"
  status=$?
  if [ "$status" -gt 128 ] && [ "$status" != 130 ] && [ "$status" != 143 ]; then
    mkdir -p "$crashes"
    echo "the host died of signal $((status - 128)) $(($(date +%s) - started)) s after starting" > "$crashes/$(date +%s)000-$child.txt"
  fi
  child=
  case $status in
    0 | 130 | 137 | 143) exit "$status" ;;
  esac
  if [ $(($(date +%s) - started)) -lt 30 ]; then quick=$((quick + 1)); else quick=0; fi
  if [ "$quick" -ge 5 ]; then
    echo "voice-vr: the host crashed $quick times in a row within 30 s of starting; giving up" >&2
    exit "$status"
  fi
  if ! pgrep -x vrserver >/dev/null; then
    echo "voice-vr: the host exited $status and vrserver is gone; not restarting" >&2
    exit "$status"
  fi
  echo "voice-vr: the host exited $status; restarting" >&2
  sleep 2
done

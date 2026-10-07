#!/bin/sh
# SteamVR launches the overlay once and never again, and the gesture and wake word live inside it, so a crash would leave nothing to summon it with.
here=$(dirname "$(readlink -f "$0")")
export VOICE_VR_SPEAKER_MODEL="${VOICE_VR_SPEAKER_MODEL:-$here/../share/speaker.onnx}"
quick=0
child=
trap 'if [ -n "$child" ]; then kill "$child" 2>/dev/null; wait "$child"; fi; exit 143' TERM INT HUP
while :; do
  started=$(date +%s)
  "$here/voice-vr-host" "$@" &
  child=$!
  wait "$child"
  status=$?
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

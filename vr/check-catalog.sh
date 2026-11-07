#!/usr/bin/env bash
# A landing whose overlay would refuse a live catalog figure fails here, before it deploys.
# The catalog is private: the picks, the errors and the host's log, which name its figures, stay in a private file this prints the path of.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
host=$(nix-build "$here" --no-out-link)/bin/voice-vr
export VK_ICD_FILENAMES
VK_ICD_FILENAMES=$(nix-build '<nixpkgs>' -A mesa --no-out-link)/share/vulkan/icd.d/lvp_icd.$(uname -m).json
# A state of its own keeps an unlanded build's crash reports out of the ones the installed host delivers; the asset cache is shared, keyed by content.
cache=${XDG_STATE_HOME:-$HOME/.local/state}/voice-vr/assets
state=$(mktemp -d)
mkdir -p "$cache" "$state/voice-vr"
ln -s "$cache" "$state/voice-vr/assets"
log=$state/load.log
if ! XDG_STATE_HOME=$state env -u OTEL_EXPORTER_OTLP_ENDPOINT "$host" load >"$state/picks.jsonl" 2>"$log"; then
  echo "voice-vr load failed; its log: $log" >&2
  exit 1
fi
[ -s "$state/picks.jsonl" ] || { echo "the live catalog served no figure; the host's log: $log" >&2; exit 1; }
refused=0
jq -es 'all(.shown)' "$state/picks.jsonl" >/dev/null || refused=$?
if [ "$refused" = 0 ]; then
  rm -r "$state"
  echo 'every live catalog figure loads'
  exit 0
fi
if [ "$refused" != 1 ]; then
  echo "voice-vr load printed unreadable picks; they and its log: $state" >&2
  exit 1
fi
jq -r 'select(.shown | not) | "\(.id): \(.error)"' "$state/picks.jsonl" >>"$log"
echo "this overlay did not load a live catalog figure; fix the overlay, land the converter that writes what it now needs, or republish the figure through scripts/publish-avatar.py with VOICE_VR=$host. The errors end $log" >&2
exit 1

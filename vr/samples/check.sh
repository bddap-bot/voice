#!/usr/bin/env bash
# Builds every sample and runs each through the overlay host's own object path on a software GPU: it must load, and its program must stay within its instruction and memory budget through touch, spread and scrub.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
out=$(mktemp -d)
trap 'rm -rf "$out"' EXIT
host=$(nix-build "$here/.." --no-out-link)/bin/voice-vr
export VK_ICD_FILENAMES
VK_ICD_FILENAMES=$(nix-build '<nixpkgs>' -A mesa --no-out-link)/share/vulkan/icd.d/lvp_icd.$(uname -m).json
nix-shell "$here/shell.nix" --run "python3 '$here/make.py' '$out'"
for object in "$out"/*.glb; do
  "$host" object --unbudgeted "$object"
done

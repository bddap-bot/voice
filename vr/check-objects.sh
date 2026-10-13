#!/usr/bin/env bash
# Runs every object of the pinned wunderkammer-objects through the overlay host's own object path on a software GPU: it must load, and its program must stay within its instruction and memory budget through touch, spread and scrub.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
host=$(nix-build "$here" --no-out-link)/bin/voice-vr
objects=$(nix-build "$here/objects.nix" --no-out-link)
export VK_ICD_FILENAMES
VK_ICD_FILENAMES=$(nix-build '<nixpkgs>' -A mesa --no-out-link)/share/vulkan/icd.d/lvp_icd.$(uname -m).json
for object in "$objects"/*.glb; do
  "$host" object --unbudgeted "$object"
done

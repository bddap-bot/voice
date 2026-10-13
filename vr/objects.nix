{ pkgs ? import <nixpkgs> { } }:
import (pkgs.fetchFromGitHub {
  owner = "bddap-bot";
  repo = "wunderkammer-objects";
  rev = "3af95a72dbf73465f3e18f3a3e11c3f6206e63af";
  sha256 = "0gc4hfyxa48gvndv4lwpalnfgw1xvpw0np9b8llpmq9cx64h9amq";
}) { inherit pkgs; }

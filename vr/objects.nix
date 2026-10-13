{ pkgs ? import <nixpkgs> { } }:
import (pkgs.fetchFromGitHub {
  owner = "bddap-bot";
  repo = "wunderkammer-objects";
  rev = "196b379e524376dd77e6bff68d8af0e7402650ba";
  sha256 = "1amrvykfwd0afxjqx3lr5hbir0znzfrr5q4r2d6ybzw8kf660yip";
}) { inherit pkgs; }

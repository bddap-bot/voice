{ pkgs ? import <nixpkgs> { } }:
rec {
  source = pkgs.fetchFromGitHub {
    owner = "bddap-bot";
    repo = "wunderkammer-objects";
    rev = "bd14fa2588f2569aed65b95deec51db63db45735";
    sha256 = "0kqhjjx13vbq33ia0llcsp39hkymdjxhsl0spzp18lzirr4dwg5x";
  };
  objects = import source { inherit pkgs; };
}

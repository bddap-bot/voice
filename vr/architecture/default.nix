{ pkgs ? import <nixpkgs> { }, descriptions ? ./ready }:
let
  source = pkgs.lib.fileset.toSource { root = ./.; fileset = pkgs.lib.fileset.unions [ ./generate.py ./program.rs ./test.mjs ]; };
  tools = {
    nativeBuildInputs = [ (pkgs.python3.withPackages (p: [ p.numpy ])) pkgs.rustc pkgs.lld pkgs.nodejs ];
    PYTHONDONTWRITEBYTECODE = 1;
    PYTHONPATH = (import ../objects.nix { inherit pkgs; }).source;
  };
  build = descriptions: extra: pkgs.runCommand "voice-architectures" (tools // extra) ''
    python3 ${source}/generate.py $out ${descriptions}/*.json
  '';
  tests = pkgs.runCommand "voice-architectures-tests" tools ''
    ARCHITECTURES=${build ./ready { }} GENERATOR=${source}/generate.py node --test ${source}/test.mjs | tee $out
  '';
in
build descriptions { passthru = { inherit tests; }; }

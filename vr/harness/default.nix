{ pkgs ? import <nixpkgs> { }, simulatedController ? import ../simulated { inherit pkgs; } }:
let
  steam = import pkgs.path { system = pkgs.stdenv.hostPlatform.system; config.allowUnfreePredicate = p: builtins.elem (pkgs.lib.getName p) [ "steam-unwrapped" "steam-run" ]; };
  clientLibraries = pkgs.lib.makeLibraryPath [ pkgs.vulkan-loader pkgs.libglvnd pkgs.libuuid ] + ":/run/opengl-driver/lib";
  scene = pkgs.stdenv.mkDerivation {
    pname = "vrh-scene";
    version = "0.1.0";
    src = pkgs.lib.fileset.toSource { root = ./.; fileset = ./vk_scene.cpp; };
    nativeBuildInputs = [ pkgs.makeWrapper ];
    buildInputs = [ pkgs.openvr pkgs.vulkan-headers pkgs.vulkan-loader ];
    buildPhase = "$CXX -std=c++17 -O2 -Wall vk_scene.cpp -lopenvr_api -lvulkan -o vrh-scene";
    installPhase = "install -Dm755 vrh-scene $out/bin/vrh-scene && wrapProgram $out/bin/vrh-scene --prefix LD_LIBRARY_PATH : ${clientLibraries}";
  };
  tools = pkgs.lib.makeBinPath [ pkgs.passt pkgs.bubblewrap pkgs.procps pkgs.gawk pkgs.coreutils pkgs.xorg.xvfb pkgs.imagemagick steam.steam-run scene ];
in
pkgs.runCommand "vrh" { nativeBuildInputs = [ pkgs.makeWrapper ]; } ''
  install -Dm644 ${./steamvr.vrsettings} $out/share/vrh/steamvr.vrsettings
  mkdir -p $out/libexec/vrh
  substitute ${./vrh} $out/libexec/vrh/vrh --subst-var-by simulatedController ${simulatedController} --subst-var-by share $out/share/vrh
  install -m755 ${./vrh-inner} $out/libexec/vrh/vrh-inner
  install -m755 ${./vrh-capture} $out/libexec/vrh/vrh-capture
  chmod 755 $out/libexec/vrh/vrh
  patchShebangs $out/libexec/vrh
  makeWrapper $out/libexec/vrh/vrh $out/bin/vrh \
    --prefix PATH : $out/libexec/vrh:${tools} \
    --set VRH_CLIENT_LD_LIBRARY_PATH ${clientLibraries}

  PATH=${pkgs.coreutils}/bin:${pkgs.gawk}/bin:$PATH bash ${./test.sh} $out/libexec/vrh/vrh
''

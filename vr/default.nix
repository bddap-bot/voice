{ pkgs ? import <nixpkgs> { } }:
let
  runtimeLibraries = pkgs.lib.makeLibraryPath [ pkgs.vulkan-loader pkgs.libglvnd pkgs.libuuid ];
in
pkgs.rustPlatform.buildRustPackage {
  pname = "voice-vr";
  version = "0.1.0";
  src = pkgs.lib.cleanSource ./.;
  cargoLock.lockFile = ./Cargo.lock;
  nativeBuildInputs = [ pkgs.cmake pkgs.rustPlatform.bindgenHook pkgs.makeWrapper ];
  dontUseCmakeConfigure = true;
  postInstall = ''
    wrapProgram $out/bin/voice-vr \
      --prefix LD_LIBRARY_PATH : ${runtimeLibraries} \
      --set-default VOICE_VR_BROWSER ${pkgs.chromium}/bin/chromium
    mkdir -p $out/share/voice-vr
    substitute ${./voice-vr.vrmanifest} $out/share/voice-vr/voice-vr.vrmanifest --subst-var out
  '';
}

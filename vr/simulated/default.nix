{ pkgs ? import <nixpkgs> { } }:
pkgs.stdenv.mkDerivation {
  pname = "voice-vr-simulated-controller";
  version = "0.1.0";
  src = pkgs.lib.fileset.toSource { root = ./.; fileset = pkgs.lib.fileset.unions [ ./controller.cpp ./driver.vrdrivermanifest ]; };
  buildInputs = [ pkgs.openvr ];
  buildPhase = ''
    $CXX -std=c++17 -shared -fPIC -fvisibility=hidden -O2 -Wall -Werror -I${pkgs.openvr}/include/openvr -static-libstdc++ -static-libgcc controller.cpp -o driver_simulated.so
  '';
  installPhase = ''
    install -Dm644 driver.vrdrivermanifest $out/driver.vrdrivermanifest
    install -Dm755 driver_simulated.so $out/bin/linux64/driver_simulated.so
  '';
}

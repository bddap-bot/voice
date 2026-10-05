{ pkgs ? import <nixpkgs> { } }:
let
  host = target: target.rustPlatform.buildRustPackage {
    pname = "voice-vr";
    version = "0.1.0";
    src = pkgs.lib.fileset.toSource {
      root = ./..;
      fileset = pkgs.lib.fileset.unions [ ./Cargo.toml ./Cargo.lock ./build.rs ./src ./shaders ./golden ../docs/poses/standing.json ../docs/identity.js ../docs/live.js ];
    };
    cargoRoot = "vr";
    buildAndTestSubdir = "vr";
    cargoLock.lockFile = ./Cargo.lock;
    nativeBuildInputs = [ target.buildPackages.cmake target.rustPlatform.bindgenHook ];
    dontUseCmakeConfigure = true;
    preCheck = ''
      export LD_LIBRARY_PATH=${pkgs.vulkan-loader}/lib
      export VK_ICD_FILENAMES=${pkgs.mesa}/share/vulkan/icd.d/lvp_icd.${pkgs.stdenv.hostPlatform.parsed.cpu.name}.json
    '';
  };
  native = host pkgs;
  aarch64 = host pkgs.pkgsCross.aarch64-multiplatform;
  runtimeLibraries = pkgs.lib.makeLibraryPath [ pkgs.vulkan-loader pkgs.libglvnd pkgs.libuuid ];
  # The newest glibc the standalone headset's SteamOS ships; the bundle runs on the system's own loader and libraries.
  deviceGlibc = "2.39";
  bundle = pkgs.runCommand "voice-vr-aarch64-bundle" { nativeBuildInputs = [ pkgs.patchelf pkgs.binutils pkgs.nukeReferences ]; allowedReferences = [ ]; } ''
    mkdir -p $out/bin
    install -m755 ${aarch64}/bin/voice-vr $out/bin/voice-vr-host
    patchelf --set-interpreter /lib/ld-linux-aarch64.so.1 --remove-rpath $out/bin/voice-vr-host
    nuke-refs $out/bin/voice-vr-host

    readelf -h $out/bin/voice-vr-host | grep -q 'Machine: *AArch64'
    needed=$(readelf -d $out/bin/voice-vr-host | sed -n 's/.*(NEEDED).*\[\(.*\)\]/\1/p' | sort | tr '\n' ' ')
    for library in $needed; do
      case $library in
        libc.so.6|libm.so.6|libgcc_s.so.1|libstdc++.so.6|ld-linux-aarch64.so.1) ;;
        *) echo "voice-vr-host needs $library, which the headset may lack" >&2; exit 1 ;;
      esac
    done
    newest=$(readelf -V $out/bin/voice-vr-host | grep -o 'GLIBC_[0-9.]*' | sed 's/GLIBC_//' | sort -V | tail -1)
    if [ "$(printf '%s\n' "$newest" ${deviceGlibc} | sort -V | tail -1)" != ${deviceGlibc} ]; then
      echo "voice-vr-host needs glibc $newest, newer than the headset's ${deviceGlibc}" >&2; exit 1
    fi
    grep -q linuxarm64 $out/bin/voice-vr-host

    cat > $out/bin/voice-vr <<'EOF'
    #!/bin/sh
    here=$(dirname "$(readlink -f "$0")")
    export VOICE_VR_BROWSER="''${VOICE_VR_BROWSER:-$here/chromium}"
    exec "$here/voice-vr-host" "$@"
    EOF
    # The Flatpak's sandboxed zygote is spawned through flatpak-portal, outside --die-with-parent, and would outlive the host.
    cat > $out/bin/chromium <<'EOF'
    #!/bin/sh
    exec flatpak run --die-with-parent org.chromium.Chromium --no-zygote --no-sandbox "$@"
    EOF
    chmod 755 $out/bin/voice-vr $out/bin/chromium
    substitute ${./voice-vr.vrmanifest} $out/voice-vr.vrmanifest --replace-fail '"binary_path_linux": "@out@/bin/voice-vr"' '"binary_path_linux_arm": "bin/voice-vr"'
  '';
in
pkgs.runCommand "voice-vr-${native.version}" { nativeBuildInputs = [ pkgs.makeWrapper ]; passthru = { inherit bundle; simulatedController = import ./simulated { inherit pkgs; }; }; } ''
  makeWrapper ${native}/bin/voice-vr $out/bin/voice-vr \
    --prefix LD_LIBRARY_PATH : ${runtimeLibraries} \
    --set-default VOICE_VR_BROWSER ${pkgs.chromium}/bin/chromium
  mkdir -p $out/share/voice-vr
  substitute ${./voice-vr.vrmanifest} $out/share/voice-vr/voice-vr.vrmanifest --subst-var out
''

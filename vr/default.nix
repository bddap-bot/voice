{ pkgs ? import <nixpkgs> { } }:
let
  pinned = builtins.match ".*url: '([^']+)',[[:space:]]*sha256: '([0-9a-f]+)'.*" (builtins.readFile ../docs/speaker.js);
  speakerModel = pkgs.fetchurl { url = builtins.elemAt pinned 0; sha256 = builtins.elemAt pinned 1; };
  # The page's action embedding model at a fixed revision; vr/golden/actions.json holds the page's decisions on it.
  actionModel = let
    id = builtins.elemAt (builtins.match ".*export const EMBEDDING_MODEL = '([^']+)';.*" (builtins.readFile ../docs/puppet-drivers.js)) 0;
    file = path: sha256: pkgs.fetchurl { url = "https://huggingface.co/${id}/resolve/751bff37182d3f1213fa05d7196b954e230abad9/${path}"; inherit sha256; };
  in pkgs.linkFarm "voice-vr-action-model" {
    "model_quantized.onnx" = file "onnx/model_quantized.onnx" "afdb6f1a0e45b715d0bb9b11772f032c399babd23bfc31fed1c170afc848bdb1";
    "tokenizer.json" = file "tokenizer.json" "da0e79933b9ed51798a3ae27893d3c5fa4a201126cef75586296df9b4d2c62a0";
  };
  # Real recordings of three speakers from the model authors' examples (Apache-2.0), at the capture rate.
  speech = pkgs.runCommand "voice-vr-speech" { nativeBuildInputs = [ pkgs.sox ]; } (''
    mkdir $out
  '' + pkgs.lib.concatStrings (pkgs.lib.mapAttrsToList (name: { path, sha256 }: ''
    sox ${pkgs.fetchurl { url = "https://raw.githubusercontent.com/csukuangfj/sr-data/92b4978ba6b4358e95be385e51e86ece86524d33/${path}"; inherit sha256; }} -r 48000 -e floating-point -b 32 -c 1 -t raw $out/${name}.f32
  '') {
    leijun-sr-1 = { path = "enroll/leijun-sr-1.wav"; sha256 = "160a3d9bf5dd5038da8191b4430e1f3f751461613ae9489401b6b35a61b488ad"; };
    leijun-sr-2 = { path = "enroll/leijun-sr-2.wav"; sha256 = "37a759c036f2520d143708dfe46d45901b38a0a6e621b52d6f1aef2c000d7fe0"; };
    leijun-test-sr-1 = { path = "test/leijun-test-sr-1.wav"; sha256 = "36cda04ee4d10e38095de73b77c99e8e7c54347232967a9cde4cf54f7d496bab"; };
    leijun-test-sr-2 = { path = "test/leijun-test-sr-2.wav"; sha256 = "84b85e7413b5348303a95bd78813e4d669b4dc70fa5a99990b9860ca6fda91e8"; };
    fangjun-test-sr-1 = { path = "test/fangjun-test-sr-1.wav"; sha256 = "9175e523081bf6a630ce72a55b05f92148eaafaf58cbbbe743686cd81c50848e"; };
    speaker2_a_en = { path = "test/3d-speaker/speaker2_a_en_16k.wav"; sha256 = "a723c134978a17fe12ca2374d0281a8003a56fa44ff9d2249a08791714983362"; };
  }));
  host = target: target.rustPlatform.buildRustPackage {
    pname = "voice-vr";
    version = "0.1.0";
    src = pkgs.lib.fileset.toSource {
      root = ./..;
      fileset = pkgs.lib.fileset.unions [ ./Cargo.toml ./Cargo.lock ./build.rs ./src ./shaders ./golden ../docs/poses/standing.json ../docs/identity.js ../docs/live.js ../docs/speaker.js ../docs/wake/melspectrogram.onnx ../docs/wake/embedding_model.onnx ../scripts/wake-demo.json ../test/fixtures/delegated-reply-capture.json ];
    };
    cargoRoot = "vr";
    buildAndTestSubdir = "vr";
    cargoLock.lockFile = ./Cargo.lock;
    nativeBuildInputs = [ target.buildPackages.cmake target.rustPlatform.bindgenHook ];
    dontUseCmakeConfigure = true;
    preCheck = ''
      export LD_LIBRARY_PATH=${pkgs.vulkan-loader}/lib
      export VK_ICD_FILENAMES=${pkgs.mesa}/share/vulkan/icd.d/lvp_icd.${pkgs.stdenv.hostPlatform.parsed.cpu.name}.json
      export VOICE_VR_SPEAKER_MODEL=${speakerModel} VOICE_VR_SPEECH=${speech} VOICE_VR_ACTION_MODEL=${actionModel}
    '';
  };
  native = host pkgs;
  aarch64 = host pkgs.pkgsCross.aarch64-multiplatform;
  runtimeLibraries = pkgs.lib.makeLibraryPath [ pkgs.vulkan-loader pkgs.libglvnd pkgs.libuuid ];
  # The newest glibc the standalone headset's SteamOS ships; the bundle runs on the system's own loader and libraries.
  deviceGlibc = "2.39";
  bundle = pkgs.runCommand "voice-vr-aarch64-bundle" { nativeBuildInputs = [ pkgs.patchelf pkgs.binutils pkgs.nukeReferences ]; allowedReferences = [ ]; } ''
    mkdir -p $out/bin $out/share
    install -m755 ${aarch64}/bin/voice-vr $out/bin/voice-vr-host
    install -m644 ${speakerModel} $out/share/speaker.onnx
    install -Dm644 -t $out/share/action ${actionModel}/model_quantized.onnx ${actionModel}/tokenizer.json
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

    install -m755 ${./voice-vr.sh} $out/bin/voice-vr
    sh ${./voice-vr-test.sh} $out/bin/voice-vr
    substitute ${./voice-vr.vrmanifest} $out/voice-vr.vrmanifest --replace-fail '"binary_path_linux": "@out@/bin/voice-vr"' '"binary_path_linux_arm": "bin/voice-vr"'
  '';
in
pkgs.runCommand "voice-vr-${native.version}" { nativeBuildInputs = [ pkgs.makeWrapper ]; passthru = rec { inherit bundle; simulatedController = import ./simulated { inherit pkgs; }; harness = import ./harness { inherit pkgs simulatedController; }; }; } ''
  makeWrapper ${native}/bin/voice-vr $out/bin/voice-vr \
    --prefix LD_LIBRARY_PATH : ${runtimeLibraries} \
    --set-default VOICE_VR_SPEAKER_MODEL ${speakerModel} \
    --set-default VOICE_VR_ACTION_MODEL ${actionModel}
  mkdir -p $out/share/voice-vr
  substitute ${./voice-vr.vrmanifest} $out/share/voice-vr/voice-vr.vrmanifest --subst-var out
''

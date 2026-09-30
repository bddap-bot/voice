#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
: "${ANDROID_HOME:?Set ANDROID_HOME to an Android SDK with platform 35 and build-tools 35.0.0}"
: "${VOICE_KEYSTORE:?Set VOICE_KEYSTORE to a private signing keystore}"
: "${VOICE_STORE_PASSWORD:?Set VOICE_STORE_PASSWORD}"
tools="$ANDROID_HOME/build-tools/35.0.0"
platform="$ANDROID_HOME/platforms/android-35/android.jar"
out=android/build
rm -rf "$out"
mkdir -p "$out/classes" "$out/dex"
javac --release 8 -classpath "$platform" -d "$out/classes" android/src/voice/live/*.java
bash "$tools/d8" --lib "$platform" --min-api 28 --output "$out/dex" "$out"/classes/voice/live/*.class
"$tools/aapt2" link -I "$platform" --manifest android/AndroidManifest.xml -o "$out/unsigned.apk"
(cd "$out/dex" && zip -q ../unsigned.apk classes.dex)
"$tools/zipalign" -f 4 "$out/unsigned.apk" "$out/aligned.apk"
bash "$tools/apksigner" sign --ks "$VOICE_KEYSTORE" --ks-pass env:VOICE_STORE_PASSWORD --out "$out/live-voice.apk" "$out/aligned.apk"
bash "$tools/apksigner" verify --verbose "$out/live-voice.apk"
if [[ "${1:-}" == --probe ]]; then
    mkdir -p "$out/probe-classes" "$out/probe-dex"
    javac --release 8 -classpath "$platform:$out/classes" -d "$out/probe-classes" android/test/Probe.java
    bash "$tools/d8" --lib "$platform" --min-api 28 --output "$out/probe-dex" "$out"/probe-classes/voice/live/*.class
    "$tools/aapt2" link -I "$platform" --manifest android/test/AndroidManifest.xml -o "$out/probe-unsigned.apk"
    (cd "$out/probe-dex" && zip -q ../probe-unsigned.apk classes.dex)
    "$tools/zipalign" -f 4 "$out/probe-unsigned.apk" "$out/probe-aligned.apk"
    bash "$tools/apksigner" sign --ks "$VOICE_KEYSTORE" --ks-pass env:VOICE_STORE_PASSWORD --out "$out/probe.apk" "$out/probe-aligned.apk"
    bash "$tools/apksigner" verify --verbose "$out/probe.apk"
fi

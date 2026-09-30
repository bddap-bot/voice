#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
: "${ANDROID_HOME:?Set ANDROID_HOME to an Android SDK with platform 35 and build-tools 35.0.0}"
tools="$ANDROID_HOME/build-tools/35.0.0"
platform="$ANDROID_HOME/platforms/android-35/android.jar"
out=android/build
apk=live-voice.apk
debug=()
if [[ "${1:-}" == --test ]]; then
    apk=live-voice-test.apk
    debug=(--debug-mode)
    work="$out/test"
    rm -rf "$work"
    mkdir -p "$work"
    VOICE_STORE_PASSWORD=$(head -c 24 /dev/urandom | base64)
    VOICE_KEYSTORE="$work/throwaway.p12"
    export VOICE_STORE_PASSWORD
    keytool -genkeypair -keystore "$VOICE_KEYSTORE" -storetype PKCS12 -storepass:env VOICE_STORE_PASSWORD -alias test -keyalg RSA -keysize 2048 -validity 1 -dname CN=test >/dev/null
else
    : "${VOICE_KEYSTORE:?Set VOICE_KEYSTORE to a private signing keystore}"
    : "${VOICE_STORE_PASSWORD:?Set VOICE_STORE_PASSWORD}"
    work="$out/release"
    rm -rf "$work"
fi
mkdir -p "$work/classes" "$work/dex"
javac --release 8 -classpath "$platform" -d "$work/classes" android/src/voice/live/*.java
bash "$tools/d8" --lib "$platform" --min-api 28 --output "$work/dex" "$work"/classes/voice/live/*.class
"$tools/aapt2" link "${debug[@]}" -I "$platform" --manifest android/AndroidManifest.xml -o "$work/unsigned.apk"
(cd "$work/dex" && zip -q ../unsigned.apk classes.dex)
"$tools/zipalign" -f 4 "$work/unsigned.apk" "$work/aligned.apk"
bash "$tools/apksigner" sign --ks "$VOICE_KEYSTORE" --ks-pass env:VOICE_STORE_PASSWORD --out "$out/$apk" "$work/aligned.apk"
bash "$tools/apksigner" verify --verbose "$out/$apk"

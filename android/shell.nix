let
  pkgs = import <nixpkgs> { config = { allowUnfree = true; android_sdk.accept_license = true; }; };
  sdk = (pkgs.androidenv.composeAndroidPackages {
    platformVersions = [ "35" ];
    buildToolsVersions = [ "35.0.0" ];
    includeEmulator = false;
    includeSources = false;
    includeSystemImages = false;
    includeNDK = false;
  }).androidsdk;
in pkgs.mkShell {
  packages = [ pkgs.jdk17_headless pkgs.zip ];
  ANDROID_HOME = "${sdk}/libexec/android-sdk";
}

{
  description = "Android build environment for Datum Tunnels (Dioxus)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, rust-overlay, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
          config = { allowUnfree = true; };
        };

        # Rust toolchain with Android + WASM targets
        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" "rust-analyzer" "rustfmt" "clippy" ];
          targets = [
            "aarch64-linux-android"
            "armv7-linux-androideabi"
            "i686-linux-android"
            "x86_64-linux-android"
            "wasm32-unknown-unknown"
          ];
        };

        # Android SDK + NDK via androidenv
        androidEnv = pkgs.callPackage "${nixpkgs}/pkgs/development/mobile/androidenv" {
          inherit pkgs;
          licenseAccepted = true;
        };

        androidComposition = androidEnv.composeAndroidPackages {
          includeSources = false;
          includeSystemImages = true;
          includeEmulator = "if-supported";
          buildToolsVersions = [ "34.0.0" ];
          includeNDK = "if-supported";
          platformVersions = [ "34" "35" "latest" ];
          # Only need the x86_64 google_apis image for this host's emulator.
          abiVersions = [ "x86_64" ];
          systemImageTypes = [ "google_apis" ];
        };

        # Wrapped emulator (bakes in ANDROID_SDK_ROOT / config).
        androidEmulator = androidEnv.emulateApp {
          name = "datum-android-emulator";
          configOptions = {
            "hw.keyboard" = "yes";
          };
          sdkExtraArgs = {
            includeSystemImages = true;
            includeEmulator = "if-supported";
            platformVersions = [ "34" "35" "latest" ];
            buildToolsVersions = [ "34.0.0" ];
            abiVersions = [ "x86_64" ];
            systemImageTypes = [ "google_apis" ];
          };
        };

        androidSdk = androidComposition.androidsdk;
        platformTools = androidComposition.platform-tools;
        androidHome = "${androidSdk}/libexec/android-sdk";
        jdk = pkgs.jdk21;
      in
      {
        devShells.android = pkgs.mkShell {
          name = "datum-android-env";
          packages = [
            androidSdk
            platformTools
            androidEmulator
            rustToolchain
            jdk
            pkgs.dioxus-cli
            pkgs.pkg-config
            pkgs.openssl
            pkgs.gcc
            pkgs.clang
            pkgs.nodejs
          ];

          LANG = "C.UTF-8";
          LC_ALL = "C.UTF-8";
          JAVA_HOME = jdk.home;

          ANDROID_HOME = androidHome;
          ANDROID_SDK_ROOT = androidHome;
          ANDROID_NDK_ROOT = "${androidHome}/ndk-bundle";
          ANDROID_NDK_HOME = "${androidHome}/ndk-bundle";
          ANDROID_NDK = "${androidHome}/ndk-bundle";

          # AVDs / emulator data must live in a writable location (Nix SDK store is read-only).
          ANDROID_USER_HOME = "/home/drewr/src/datum-cloud/android-worktree/.android";
          ANDROID_AVD_HOME = "/home/drewr/src/datum-cloud/android-worktree/.android/avd";

          RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";

          shellHook = ''
            export PATH="$ANDROID_HOME/platform-tools:$ANDROID_HOME/cmdline-tools/latest/bin:$ANDROID_HOME/emulator:$PATH"
            mkdir -p "$ANDROID_AVD_HOME"
            echo "🚀 Android build environment loaded"
            echo "  ANDROID_HOME=$ANDROID_HOME"
            echo "  ANDROID_NDK_HOME=$ANDROID_NDK_HOME"
            echo "  JAVA_HOME=$JAVA_HOME"
            echo "  ANDROID_USER_HOME=$ANDROID_USER_HOME"
            echo "  rustup targets: $(rustup target list --installed 2>/dev/null | tr '\n' ' ')"
            echo "  dx: $(dx --version 2>/dev/null | head -1 || echo 'not found')"
          '';
        };
      }
    );
}

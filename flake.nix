{
  description = "Dioxus development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, rust-overlay, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };

        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" "rust-analyzer" ];
          targets = [ "wasm32-unknown-unknown" ];
        };

        # Toolchain used for the static (musl) CLI build. Kept separate from
        # the dev toolchain so the dev shell isn't forced to download musl
        # targets it doesn't need.
        muslTarget =
          if pkgs.stdenv.hostPlatform.isAarch64 then "aarch64-unknown-linux-musl"
          else "x86_64-unknown-linux-musl";

        rustMuslToolchain = pkgs.rust-bin.stable.latest.default.override {
          targets = [ muslTarget ];
        };

        rustMuslPlatform = pkgs.makeRustPlatform {
          cargo = rustMuslToolchain;
          rustc = rustMuslToolchain;
        };

        # Platform-specific packages
        darwinPackages = with pkgs; lib.optionals stdenv.isDarwin [
          apple-sdk_15
        ];

        linuxPackages = with pkgs; lib.optionals stdenv.isLinux [
          # For web/desktop rendering
          webkitgtk_4_1
          gtk3
          libsoup_3
          # X11 dependencies
          xdotool
          xorg.libX11
          xorg.libXcursor
          xorg.libXrandr
          xorg.libXi
        ];

        cargoOutputHashes = {
          "dioxus-primitives-0.0.1" = "sha256-T/ZdVqgWDLpdNzf3GlBeQVLbs4eJbqdgDkrUSzMycR4=";
        };

      in
      {
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "datum-connect";
          version = "0.1.0";

          src = ./.;

          cargoLock = {
            lockFile = ./Cargo.lock;
            outputHashes = cargoOutputHashes;
          };

          nativeBuildInputs = with pkgs; [
            pkg-config
          ];

          buildInputs = with pkgs; [
            openssl
          ] ++ lib.optionals stdenv.isDarwin [
            libiconv
          ];

          cargoBuildFlags = [ "--workspace" ];
          doCheck = false; # tests require network (iroh STUN/relay); run with `cargo test` locally

          meta = with pkgs.lib; {
            description = "Datum Connect - A tunneling solution built on iroh";
            homepage = "https://github.com/datum-cloud/datum-connect";
            license = licenses.agpl3Only;
            maintainers = [ ];
          };
        };

        packages.cli = pkgs.rustPlatform.buildRustPackage {
          pname = "datum-connect-cli";
          version = "0.1.0";

          src = ./.;

          cargoLock = {
            lockFile = ./Cargo.lock;
            outputHashes = cargoOutputHashes;
          };

          nativeBuildInputs = with pkgs; [
            pkg-config
          ];

          buildInputs = with pkgs; [
            openssl
          ] ++ lib.optionals stdenv.isDarwin [
            libiconv
          ];

          cargoBuildFlags = [ "-p" "datum-connect" ];
          doCheck = false; # tests require network (iroh STUN/relay); run with `cargo test` locally

          meta = with pkgs.lib; {
            description = "Datum Connect CLI";
            mainProgram = "datum-connect";
          };
        };

        # Fully statically linked Linux CLI (musl + static openssl).
        # Build with: nix build .#cli-static
        #
        # We host-build with the default gnu stdenv and cross-target musl
        # via cargo's --target flag. Earlier versions of this derivation
        # tried `.override { stdenv = muslPkgs.stdenv }` so that
        # cargoBuildHook would auto-pick --target=...-musl, but the
        # combination of makeRustPlatform + stdenv override + the install
        # hook ended up writing the gnu-built binary into $out/bin (only
        # the store path name carried the "-musl" suffix). Driving the
        # build and install phases ourselves removes that ambiguity.
        packages.cli-static = let
          muslPkgs =
            if pkgs.stdenv.hostPlatform.isAarch64 then pkgs.pkgsCross.aarch64-multiplatform-musl
            else pkgs.pkgsCross.musl64;
          muslCC = muslPkgs.stdenv.cc;
          muslOpenssl = muslPkgs.pkgsStatic.openssl;
          linkerEnvVar =
            "CARGO_TARGET_" + (pkgs.lib.toUpper (builtins.replaceStrings [ "-" ] [ "_" ] muslTarget)) + "_LINKER";
        in rustMuslPlatform.buildRustPackage {
          pname = "datum-connect-cli-static";
          version = "0.1.0";

          src = ./.;

          cargoLock = {
            lockFile = ./Cargo.lock;
            outputHashes = cargoOutputHashes;
          };

          nativeBuildInputs = [
            pkgs.pkg-config
            muslCC
          ];

          # openssl-sys is pulled in transitively (sentry -> native-tls).
          # Provide the musl-built static openssl so the C link succeeds.
          buildInputs = [ muslOpenssl ];

          # Tell openssl-sys + pkg-config to statically link, and point them
          # at the musl-built openssl (not the host one).
          OPENSSL_STATIC = "1";
          OPENSSL_LIB_DIR = "${muslOpenssl.out}/lib";
          OPENSSL_INCLUDE_DIR = "${muslOpenssl.dev}/include";
          PKG_CONFIG_ALL_STATIC = "1";

          # Force the C runtime to be linked statically against musl.
          CARGO_BUILD_RUSTFLAGS = "-C target-feature=+crt-static";

          # Use the musl gcc as the linker for the musl target.
          ${linkerEnvVar} = "${muslCC}/bin/${muslCC.targetPrefix}cc";

          buildPhase = ''
            runHook preBuild
            cargo build -j $NIX_BUILD_CORES \
              --target ${muslTarget} \
              --frozen \
              --release \
              -p datum-connect
            runHook postBuild
          '';

          installPhase = ''
            runHook preInstall
            install -Dm755 target/${muslTarget}/release/datum-connect \
              $out/bin/datum-connect
            runHook postInstall
          '';

          doCheck = false;

          # The musl-static binary has no nix-store deps to rewrite.
          dontPatchELF = true;

          meta = with pkgs.lib; {
            description = "Datum Connect CLI (statically linked)";
            mainProgram = "datum-connect";
            platforms = platforms.linux;
          };
        };

        devShells.default = pkgs.mkShell {
          buildInputs = with pkgs; [
            # Rust toolchain with WASM support
            rustToolchain

            # Dioxus CLI for hot reloading and bundling
            dioxus-cli

            # Build tools
            pkg-config
            openssl

            # npm
            nodejs

            # For serving web apps locally
            simple-http-server

            # Useful tools
            cargo-watch
            cargo-edit
          ] ++ darwinPackages ++ linuxPackages;

          # Environment variables
          RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";

          RUSTFLAGS = pkgs.lib.optionalString pkgs.stdenv.isLinux
            "-L${pkgs.xdotool}/lib";

          shellHook = ''
            echo "🚀 Dioxus development environment loaded"
            echo "  rustc: $(rustc --version)"
            echo "  cargo: $(cargo --version)"
            echo "  dx: $(dx --version 2>/dev/null || echo 'not found')"
            echo ""
            echo "Quick start:"
            echo "  dx new myapp      # Create new project"
            echo "  dx serve          # Start dev server with hot reload"
            echo "  dx build --release # Build for production"
          '';
        };

        formatter = pkgs.nixpkgs-fmt;

        apps.desktop = let
          script = pkgs.writeShellScriptBin "datum-desktop" ''
            cd "$PWD/ui"
            export DATUM_CONNECT_PUBLISH_TICKETS=1
            export RUST_LOG=info,lib::heartbeat=debug,lib::tunnels=debug
            exec ${pkgs.dioxus-cli}/bin/dx serve --platform desktop
          '';
        in {
          type = "app";
          program = "${script}/bin/datum-desktop";
        };

        apps.cli = let
          script = pkgs.writeShellScriptBin "datum-connect-cli" ''
            exec ${self.packages.${system}.cli}/bin/datum-connect "$@"
          '';
        in {
          type = "app";
          program = "${script}/bin/datum-connect-cli";
        };
      }
    );
}

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

        # Platform-specific packages
        darwinPackages = with pkgs; lib.optionals stdenv.isDarwin [
          apple-sdk_15
        ];

        linuxPackages = with pkgs; lib.optionals stdenv.isLinux [
          # For web/desktop rendering
          webkitgtk
          gtk3
          libsoup
          # X11 dependencies
          xorg.libX11
          xorg.libXcursor
          xorg.libXrandr
          xorg.libXi
        ];

      in
      {
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "datum-connect";
          version = "0.1.0";

          src = ./.;

          cargoLock = {
            lockFile = ./Cargo.lock;
            outputHashes = {
              "iroh-proxy-utils-0.1.0" = "sha256-DRFxQusoBIh3IaYS2AlIbsKszNQuph5Xsm2h8n4Fkw8=";
            };
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
            outputHashes = {
              "iroh-proxy-utils-0.1.0" = "sha256-DRFxQusoBIh3IaYS2AlIbsKszNQuph5Xsm2h8n4Fkw8=";
            };
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

          meta = with pkgs.lib; {
            description = "Datum Connect CLI";
            mainProgram = "datum-connect";
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
            
            # For serving web apps locally
            simple-http-server
            
            # Useful tools
            cargo-watch
            cargo-edit
          ] ++ darwinPackages ++ linuxPackages;

          # Environment variables
          RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";
          
          # For OpenSSL on macOS
          PKG_CONFIG_PATH = "${pkgs.openssl.dev}/lib/pkgconfig";
          
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
      }
    );
}

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
      }
    );
}

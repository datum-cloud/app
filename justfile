# Update all outputHashes in flake.nix after Rust git dependencies change.
# Compiles bin/update-nix-hash.rs with rustc (no Cargo.toml needed) then runs it.
# Optional argument: nix target (default: .#default).
#   just update-nix-hash
#   just update-nix-hash .#cli
update-nix-hash *args:
    rustc bin/update-nix-hash.rs -o /tmp/update-nix-hash
    /tmp/update-nix-hash {{args}}

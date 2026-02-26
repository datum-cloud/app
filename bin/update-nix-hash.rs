// Command update-nix-hash automatically updates crate output hashes in flake.nix
// after Rust git dependencies have changed.
//
// Usage (no Cargo.toml needed):
//
//   rustc bin/update-nix-hash.rs -o /tmp/update-nix-hash && /tmp/update-nix-hash
//   rustc bin/update-nix-hash.rs -o /tmp/update-nix-hash && /tmp/update-nix-hash .#cli
//
// Or via the task runner:
//
//   task update-nix-hash
//
// The script finds every  "crate-name-version" = "sha256-…"  entry inside
// outputHashes blocks of flake.nix, then updates each stale hash by:
//   1. Replacing that one hash with a fake value to provoke a Nix error.
//   2. Reading the correct hash from Nix's "got:" output.
//   3. Rewriting flake.nix in place with the correct value.
//
// Hashes are processed one at a time so that Nix always reports exactly one
// "got:" line, avoiding the ordering ambiguity that arises when all FODs are
// replaced at once and Nix builds them in parallel.

use std::io::{self, Write as _};
use std::process::{Command, Stdio};
use std::{env, fs};

const FLAKE_FILE: &str = "flake.nix";
const FAKE_HASH: &str = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

fn main() {
    // Optional positional argument: nix target.  Default: ".#default".
    let args: Vec<String> = env::args().collect();
    let target = args
        .iter()
        .skip(1)
        .find(|a| !a.starts_with('-'))
        .map(String::as_str)
        .unwrap_or(".#default")
        .to_owned();

    if fs::metadata(FLAKE_FILE).is_err() {
        die(&format!(
            "Error: {FLAKE_FILE} not found. Run this from the project root."
        ));
    }

    // Read the file once; subsequent iterations read back what was last written.
    let original = read_flake();
    let entries = find_output_hashes(&original);

    if entries.is_empty() {
        die(&format!(
            "Error: No outputHashes entries found in {FLAKE_FILE}"
        ));
    }

    for (key, hash) in &entries {
        eprintln!("  found: {key} = {hash}");
    }

    // Deduplicate by hash value so we run `nix build` once per unique source,
    // not once per package × crate combination.
    let mut seen: Vec<String> = Vec::new();
    let mut unique: Vec<(String, String)> = Vec::new();
    for (key, hash) in &entries {
        if !seen.contains(hash) {
            seen.push(hash.clone());
            unique.push((key.clone(), hash.clone()));
        }
    }

    let mut any_changed = false;

    for (crate_key, old_hash) in &unique {
        // Read whatever the file looks like after previous iterations.
        let current = read_flake();

        // Swap ONLY this hash with the fake value and write it out.
        let patched = current.replace(old_hash.as_str(), FAKE_HASH);
        write_flake(&patched);

        eprintln!("\nProbing hash for {crate_key}…");
        let output = Command::new("nix")
            .args(["build", &target])
            .stdout(Stdio::inherit())
            .stderr(Stdio::piped())
            .output()
            .unwrap_or_else(|e| die(&format!("Failed to run nix build: {e}")));

        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let _ = io::stderr().write_all(stderr.as_bytes());

        match extract_hash(&stderr) {
            Some(correct_hash) if &correct_hash == old_hash => {
                eprintln!("  {crate_key}: already up to date ({old_hash})");
                // Restore this hash (undo the FAKE swap).
                write_flake(&current);
            }
            Some(correct_hash) => {
                eprintln!("  {crate_key}: {old_hash}  →  {correct_hash}");
                // Replace old_hash everywhere in the file (covers all packages).
                let updated = current.replace(old_hash.as_str(), &correct_hash);
                write_flake(&updated);
                any_changed = true;
            }
            None => {
                // Nix didn't report a mismatch — likely the source is cached and
                // the hash was already correct.  Restore and move on.
                eprintln!("  {crate_key}: no mismatch reported (hash appears correct)");
                write_flake(&current);
            }
        }
    }

    if !any_changed {
        eprintln!("\n✓ All outputHashes are already up to date");
        return;
    }

    // Final verification build.
    eprintln!("\nVerifying build with updated hashes…");
    let status = Command::new("nix")
        .args(["build", &target])
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .unwrap_or_else(|e| die(&format!("Failed to run nix build: {e}")));

    if !status.success() {
        die("Error: Build failed with new hashes");
    }

    eprintln!("✓ Build successful with updated hashes");
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse every  `"crate-name-version" = "sha256-…";`  line from the file.
/// Returns (crate_key, hash) pairs in file order (may include duplicates when
/// the same crate appears under multiple packages).
fn find_output_hashes(content: &str) -> Vec<(String, String)> {
    let mut results = Vec::new();
    for line in content.lines() {
        let t = line.trim();
        // Match:  "some-crate-1.2.3" = "sha256-…";
        if let Some(eq_pos) = t.find("\" = \"sha256-") {
            if t.starts_with('"') {
                let key = &t[1..eq_pos];
                let after_eq = &t[eq_pos + 5..]; // skip `" = "`
                if let Some(hash_end) = after_eq.find('"') {
                    let hash = &after_eq[..hash_end];
                    if hash.starts_with("sha256-") {
                        results.push((key.to_owned(), hash.to_owned()));
                    }
                }
            }
        }
    }
    results
}

/// Extract the first `got:    sha256-…` hash from nix's stderr.
fn extract_hash(output: &str) -> Option<String> {
    for line in output.lines() {
        let t = line.trim();
        if t.starts_with("got:") {
            let hash = t["got:".len()..].trim();
            if hash.starts_with("sha256-") {
                return Some(hash.to_owned());
            }
        }
    }
    None
}

fn read_flake() -> String {
    fs::read_to_string(FLAKE_FILE)
        .unwrap_or_else(|e| die(&format!("Error reading {FLAKE_FILE}: {e}")))
}

fn write_flake(content: &str) {
    fs::write(FLAKE_FILE, content)
        .unwrap_or_else(|e| die(&format!("Error writing {FLAKE_FILE}: {e}")));
}

fn die(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(1);
}

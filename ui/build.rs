// Re-run the build when `SENTRY_DSN` changes so the value baked into the
// binary via `option_env!("SENTRY_DSN")` in `src/main.rs` stays in sync.
// Without this, cargo will happily reuse a cached artifact even after the
// env var changes.
fn main() {
    println!("cargo:rerun-if-env-changed=SENTRY_DSN");
}

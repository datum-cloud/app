//! Shared [`User-Agent`](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/User-Agent) for
//! outbound HTTP from Datum Desktop (reqwest and kube). Helps backend logs and support correlate
//! traffic with app builds.
//!
//! The version is [`env!("CARGO_PKG_VERSION")`] for this crate; keep `lib`’s version aligned with
//! the shipped desktop app when cutting releases.

/// Product token plus version, OS, and CPU arch for support and debugging.
pub fn datum_http_user_agent() -> String {
    format!(
        "Datum Desktop/{} ({}; {})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
    )
}

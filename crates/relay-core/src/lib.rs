//! Provider-agnostic Relay core.
//!
//! M0 intentionally keeps this crate small. Application behavior begins in later
//! milestones once the workspace boundary is established.

/// Returns the Relay Core crate version.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_crate_version() {
        assert_eq!(version(), "0.1.0");
    }
}

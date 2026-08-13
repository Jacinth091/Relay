//! Provider-agnostic Relay core.
//!
//! M1 introduces only the minimal application state needed by the first
//! presentation client. Presentation-framework types do not belong in this
//! crate.

/// Minimal Relay application state.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Relay {
    configured_projects: usize,
}

impl Relay {
    /// Creates Relay with deterministic empty state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the number of configured projects currently visible to Relay.
    ///
    /// M1 intentionally provides no project loading or mutation behavior; that
    /// belongs to the Project Registry milestone.
    pub fn configured_project_count(&self) -> usize {
        self.configured_projects
    }
}

/// Returns the Relay Core crate version.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initializes_with_deterministic_empty_state() {
        let relay = Relay::new();

        assert_eq!(relay.configured_project_count(), 0);
        assert_eq!(relay, Relay::default());
    }

    #[test]
    fn exposes_crate_version() {
        assert_eq!(version(), "0.1.0");
    }
}

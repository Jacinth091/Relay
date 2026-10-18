//! Herdr interactive runtime integration.
//!
//! Security rule: invoke the Herdr CLI with argv only; never use a shell or
//! build `send-text` from task text.

pub mod backplane;
pub mod binding;
pub mod cli;
pub mod client;
pub mod fake;
pub mod topology;

pub use backplane::HerdrBackplane;
pub use binding::RuntimeBindingStore;
pub use cli::{
    HerdrCli, HerdrCliPaneInfo, HerdrCommandOutput, HerdrCommandRunner, HerdrCompatibility,
    HerdrOps, HerdrPaneProcess, HerdrProcessInfo, HerdrSplitDirection, HerdrTab, HerdrTabCreation,
    HerdrVersion, HerdrWorkspace, HerdrWorkspaceCreation, ProcessHerdrRunner, TrustedToken,
    check_compatibility,
};
pub use client::HerdrClientError;
pub use fake::FakeHerdr;
pub use topology::{HerdrTopologyPlan, HerdrTopologyPlanner};

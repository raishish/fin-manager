//! Ledger, storage, import pipeline, categorisation, views, queries and the output contract.

pub mod backup;
pub mod categories;
pub mod categorize;
pub mod connectors;
pub mod contract;
pub mod crash;
pub mod deletion;
pub mod model;
pub mod pipeline;
pub mod query;
pub mod recurring;
pub mod storage;
pub mod transfers;
pub mod unlock;
pub mod views;

// The fake bridge relaxes the SimpleFIN connector's public-host rule for tests. It must
// never reach a shipped build: optimised builds with it don't compile. (So
// `cargo test --release -p fm-app` doesn't either; CI runs those tests in debug.)
#[cfg(all(feature = "fake-bridge", not(debug_assertions)))]
compile_error!("fm-core's `fake-bridge` feature is for tests only");

/// Whether the SimpleFIN connector accepts the tests' fake bridge (the `fake-bridge`
/// feature, T5.13). Always false in a shipped build.
pub const FAKE_BRIDGE: bool = cfg!(feature = "fake-bridge");

/// Crate name, used by the workspace smoke test.
pub const NAME: &str = env!("CARGO_PKG_NAME");

#[cfg(test)]
mod tests {
    #[test]
    fn crate_name_is_set() {
        assert_eq!(super::NAME, "fm-core");
    }
}

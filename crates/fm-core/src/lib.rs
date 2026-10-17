//! Ledger, storage, import pipeline, categorisation, views, queries and the output contract.

pub mod backup;
pub mod categorize;
pub mod contract;
pub mod crash;
pub mod model;
pub mod pipeline;
pub mod storage;
pub mod transfers;
pub mod unlock;

/// Crate name, used by the workspace smoke test.
pub const NAME: &str = env!("CARGO_PKG_NAME");

#[cfg(test)]
mod tests {
    #[test]
    fn crate_name_is_set() {
        assert_eq!(super::NAME, "fm-core");
    }
}

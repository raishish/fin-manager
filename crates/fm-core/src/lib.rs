//! Ledger, storage, import pipeline, categorisation, views, queries and the output contract.

pub mod model;
pub mod storage;

/// Crate name, used by the workspace smoke test.
pub const NAME: &str = env!("CARGO_PKG_NAME");

#[cfg(test)]
mod tests {
    #[test]
    fn crate_name_is_set() {
        assert_eq!(super::NAME, "fm-core");
    }
}

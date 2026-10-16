//! OFX 1.x/2.x and QFX bank-export parsing.

/// Crate name, used by the workspace smoke test.
pub const NAME: &str = env!("CARGO_PKG_NAME");

#[cfg(test)]
mod tests {
    #[test]
    fn crate_name_is_set() {
        assert_eq!(super::NAME, "fm-import-ofx");
    }
}

use serde::{Deserialize, Serialize};

macro_rules! id_type {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        ///
        /// Opaque within one ledger: storage assigns it (an SQLite rowid), and it means
        /// nothing outside that ledger. Serialised as a plain integer.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub i64);
    };
}

id_type!(
    /// Identifies an [`Account`](super::Account).
    AccountId
);
id_type!(
    /// Identifies a [`RawImport`](super::RawImport).
    ImportId
);
id_type!(
    /// Identifies a [`Transaction`](super::Transaction).
    TransactionId
);
id_type!(
    /// Identifies a [`CategoryGroup`](super::CategoryGroup).
    CategoryGroupId
);
id_type!(
    /// Identifies a [`Category`](super::Category).
    CategoryId
);
id_type!(
    /// Identifies a [`Decision`](super::Decision).
    DecisionId
);
id_type!(
    /// Identifies a [`Rule`](super::Rule).
    RuleId
);

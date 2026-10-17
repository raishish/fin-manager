//! The repository: reads and writes the T0.5 model types in the ledger tables
//! (migration 2, `schema/v2_ledger.sql`).
//!
//! Every write keeps the ids the caller gives (use [`Unlocked::next_id`] for new rows).
//! Every read rebuilds validated model types, so a row that breaks a model invariant is
//! reported as [`StorageError::InvalidRow`] instead of being passed on.

use std::collections::HashSet;

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::db::Permit;
use super::{StorageError, Unlocked};
use crate::model::{
    Account, AccountId, AmountMatch, Assignment, Bucket, BuiltinLogic, Category, CategoryGroup,
    CategoryGroupId, CategoryId, Confidence, Currency, Date, Decision, DecisionId, DecisionSource,
    DecisionState, ImportId, Ledger, ModelProvenance, Money, RawImport, Rule, RuleAction, RuleId,
    RuleMatcher, RuleOrigin, Sha256Hash, SourceRef, Taxonomy, TaxonomyVersion, TextMatch,
    Timestamp, Transaction, TransactionId,
};

/// The category list every new ledger starts with (T2.1).
const DEFAULT_TAXONOMY_JSON: &str = include_str!("../../assets/taxonomy/default-v1.json");

/// The tables that [`Unlocked::next_id`] can allocate ids in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Table {
    Accounts,
    RawImports,
    Transactions,
    CategoryGroups,
    Categories,
    Decisions,
    Rules,
}

impl Table {
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Accounts => "accounts",
            Self::RawImports => "raw_imports",
            Self::Transactions => "transactions",
            Self::CategoryGroups => "category_groups",
            Self::Categories => "categories",
            Self::Decisions => "decisions",
            Self::Rules => "rules",
        }
    }
}

/// What a deletion removed (SPEC decision 28: everything derived from what was asked
/// for), or would remove, for a dry run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DeletedCounts {
    pub transactions: u64,
    pub decisions: u64,
    pub rules: u64,
    pub imports: u64,
    pub accounts: u64,
    /// Rows recorded as deleted out of imports that were kept (SPEC decision 43).
    pub recorded_rows: u64,
}

/// Which transactions a [`DeletionPlan`] removes: every transaction, narrowed to one
/// account and to an inclusive date range when given. A transaction's date is its
/// purchase date when the bank gives one, otherwise its posted date (SPEC decision 42).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct DeletionScope {
    pub(crate) account: Option<AccountId>,
    pub(crate) from: Option<Date>,
    pub(crate) to: Option<Date>,
}

/// What one call to [`Unlocked::delete_planned`] removes, in this order: the
/// transactions in scope, the listed imports (whole, with their transactions), then the
/// listed accounts. Which files and accounts go with a scope is policy, decided above
/// storage (SPEC decision 43); the plan only says what to remove.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct DeletionPlan {
    pub(crate) transactions: Option<DeletionScope>,
    pub(crate) imports: Vec<ImportId>,
    pub(crate) accounts: Vec<AccountId>,
}

impl DeletionPlan {
    fn check(&self) -> Result<(), StorageError> {
        if self.transactions.is_none() && self.imports.is_empty() && self.accounts.is_empty() {
            return Err(StorageError::InvalidRequest("the deletion plan is empty"));
        }
        if let Some(DeletionScope {
            from: Some(from),
            to: Some(to),
            ..
        }) = self.transactions
            && from > to
        {
            return Err(StorageError::InvalidRequest(
                "the date range ends before it starts",
            ));
        }
        let imports: HashSet<_> = self.imports.iter().collect();
        let accounts: HashSet<_> = self.accounts.iter().collect();
        if imports.len() != self.imports.len() || accounts.len() != self.accounts.len() {
            return Err(StorageError::InvalidRequest(
                "the deletion plan lists an id twice",
            ));
        }
        Ok(())
    }
}

/// The row counts a [`DeletedCounts`] is the difference of.
fn table_counts(conn: &Connection) -> Result<DeletedCounts, StorageError> {
    Ok(DeletedCounts {
        transactions: count(conn, "transactions")?,
        decisions: count(conn, "decisions")?,
        rules: count(conn, "rules")?,
        imports: count(conn, "raw_imports")?,
        accounts: count(conn, "accounts")?,
        recorded_rows: 0,
    })
}

/// The default category list shipped with the app.
pub fn default_taxonomy() -> Taxonomy {
    serde_json::from_str(DEFAULT_TAXONOMY_JSON).expect("the shipped default taxonomy is valid JSON")
}

// ---- Enum <-> text, through the model's serde names, so the database uses exactly the
// ---- strings documented for the JSON form.

fn enum_text<T: Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(s)) => s,
        _ => unreachable!("unit enum variants serialise as strings"),
    }
}

fn text_enum<T: DeserializeOwned>(table: &'static str, text: String) -> Result<T, StorageError> {
    serde_json::from_value(serde_json::Value::String(text)).map_err(|_| StorageError::InvalidRow {
        table,
        reason: "unknown enum value",
    })
}

fn invalid(table: &'static str, reason: &'static str) -> StorageError {
    StorageError::InvalidRow { table, reason }
}

fn date_text(d: Date) -> String {
    d.to_string()
}

fn parse_date(table: &'static str, s: String) -> Result<Date, StorageError> {
    s.parse().map_err(|_| invalid(table, "invalid date"))
}

fn currency(table: &'static str, code: String) -> Result<Currency, StorageError> {
    Currency::new(&code).map_err(|_| invalid(table, "unknown currency"))
}

fn hash(table: &'static str, bytes: Vec<u8>) -> Result<Sha256Hash, StorageError> {
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| invalid(table, "hash is not 32 bytes"))?;
    Ok(Sha256Hash::from_bytes(bytes))
}

/// Writes refused by the protection triggers become [`StorageError::ForbiddenChange`].
fn map_write_error(e: rusqlite::Error) -> StorageError {
    if let rusqlite::Error::SqliteFailure(_, Some(message)) = &e {
        if message.contains("append-only") {
            return StorageError::ForbiddenChange("decisions are append-only");
        }
        if message.contains("pending transaction") {
            return StorageError::ForbiddenChange("only a pending transaction may become posted");
        }
        if message.contains("immutable") {
            return StorageError::ForbiddenChange("the ledger never rewrites this row");
        }
    }
    StorageError::Database(e)
}

// ---- Bucket columns: (bucket, bucket_category_id, bucket_transaction_id).

fn bucket_columns(b: &Bucket) -> (String, Option<i64>, Option<i64>) {
    let kind = match b {
        Bucket::Category { .. } => "category",
        Bucket::Transfer { .. } => "transfer",
        Bucket::Income => "income",
        Bucket::Refund { .. } => "refund",
        Bucket::NeedsReview => "needs_review",
    };
    let (category, transaction) = match *b {
        Bucket::Category { category_id } => (Some(category_id.0), None),
        Bucket::Transfer { counterpart } => (None, counterpart.map(|t| t.0)),
        Bucket::Refund {
            category_id,
            original,
        } => (Some(category_id.0), original.map(|t| t.0)),
        Bucket::Income | Bucket::NeedsReview => (None, None),
    };
    (kind.to_owned(), category, transaction)
}

fn bucket_from_columns(
    table: &'static str,
    kind: &str,
    category: Option<i64>,
    transaction: Option<i64>,
) -> Result<Bucket, StorageError> {
    let category = || {
        category
            .map(CategoryId)
            .ok_or(invalid(table, "bucket needs a category"))
    };
    Ok(match kind {
        "category" => Bucket::Category {
            category_id: category()?,
        },
        "transfer" => Bucket::Transfer {
            counterpart: transaction.map(TransactionId),
        },
        "income" => Bucket::Income,
        "refund" => Bucket::Refund {
            category_id: category()?,
            original: transaction.map(TransactionId),
        },
        "needs_review" => Bucket::NeedsReview,
        _ => return Err(invalid(table, "unknown bucket")),
    })
}

// ---- Accounts

const ACCOUNT_COLUMNS: &str =
    "id, name, kind, currency, institution, masked_number, external_id, own_account, created_at";

fn insert_account(conn: &Connection, a: &Account) -> Result<(), StorageError> {
    conn.prepare(&format!(
        "INSERT INTO accounts ({ACCOUNT_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"
    ))?
    .execute(params![
        a.id.0,
        a.name,
        enum_text(&a.kind),
        a.currency.code(),
        a.institution,
        a.masked_number,
        a.external_id,
        a.own_account,
        a.created_at.unix_seconds(),
    ])?;
    Ok(())
}

fn account_from_row(r: &Row<'_>) -> Result<Account, StorageError> {
    const T: &str = "accounts";
    Ok(Account {
        id: AccountId(r.get(0)?),
        name: r.get(1)?,
        kind: text_enum(T, r.get(2)?)?,
        currency: currency(T, r.get(3)?)?,
        institution: r.get(4)?,
        masked_number: r.get(5)?,
        external_id: r.get(6)?,
        own_account: r.get(7)?,
        created_at: Timestamp::from_unix_seconds(r.get(8)?),
    })
}

// ---- Raw imports

const RAW_IMPORT_COLUMNS: &str =
    "id, source, file_name, mapping_profile, account_id, imported_at, sha256, bytes";

fn insert_raw_import(conn: &Connection, r: &RawImport) -> Result<(), StorageError> {
    conn.prepare(&format!(
        "INSERT INTO raw_imports ({RAW_IMPORT_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"
    ))?
    .execute(params![
        r.id.0,
        enum_text(&r.source),
        r.file_name,
        r.mapping_profile,
        r.account_id.map(|a| a.0),
        r.imported_at.unix_seconds(),
        r.sha256().as_bytes().as_slice(),
        r.bytes(),
    ])?;
    Ok(())
}

fn raw_import_from_row(r: &Row<'_>) -> Result<RawImport, StorageError> {
    const T: &str = "raw_imports";
    // from_parts re-checks that the stored hash matches the stored bytes.
    let mut raw = RawImport::from_parts(
        ImportId(r.get(0)?),
        text_enum(T, r.get(1)?)?,
        Timestamp::from_unix_seconds(r.get(5)?),
        hash(T, r.get(6)?)?,
        r.get(7)?,
    )
    .map_err(|_| invalid(T, "bytes do not match their SHA-256"))?;
    raw.file_name = r.get(2)?;
    raw.mapping_profile = r.get(3)?;
    raw.account_id = r.get::<_, Option<i64>>(4)?.map(AccountId);
    Ok(raw)
}

// ---- Transactions

const TRANSACTION_COLUMNS: &str = "id, account_id, import_id, source_index, external_id, status, \
     posted_date, transaction_date, amount_minor, currency, original_amount_minor, \
     original_currency, description, payee, memo, trn_type, check_number";

fn insert_transaction(conn: &Connection, t: &Transaction) -> Result<(), StorageError> {
    conn.prepare(&format!(
        "INSERT INTO transactions ({TRANSACTION_COLUMNS}) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)"
    ))?
    .execute(params![
        t.id.0,
        t.account_id.0,
        t.source.import_id.0,
        t.source.index,
        t.source.external_id,
        enum_text(&t.status),
        date_text(t.posted_date),
        t.transaction_date.map(date_text),
        t.amount.minor(),
        t.amount.currency().code(),
        t.original_amount.map(Money::minor),
        t.original_amount.map(|m| m.currency().code()),
        t.description,
        t.payee,
        t.memo,
        t.trn_type,
        t.check_number,
    ])?;
    Ok(())
}

fn transaction_from_row(r: &Row<'_>) -> Result<Transaction, StorageError> {
    const T: &str = "transactions";
    let original_amount = match (
        r.get::<_, Option<i64>>(10)?,
        r.get::<_, Option<String>>(11)?,
    ) {
        (Some(minor), Some(code)) => Some(Money::new(minor, currency(T, code)?)),
        (None, None) => None,
        _ => return Err(invalid(T, "original amount without its currency")),
    };
    Ok(Transaction {
        id: TransactionId(r.get(0)?),
        account_id: AccountId(r.get(1)?),
        source: SourceRef {
            import_id: ImportId(r.get(2)?),
            index: r.get(3)?,
            external_id: r.get(4)?,
        },
        status: text_enum(T, r.get(5)?)?,
        posted_date: parse_date(T, r.get(6)?)?,
        transaction_date: r
            .get::<_, Option<String>>(7)?
            .map(|s| parse_date(T, s))
            .transpose()?,
        amount: Money::new(r.get(8)?, currency(T, r.get(9)?)?),
        original_amount,
        description: r.get(12)?,
        payee: r.get(13)?,
        memo: r.get(14)?,
        trn_type: r.get(15)?,
        check_number: r.get(16)?,
    })
}

// ---- Taxonomy

fn save_taxonomy(conn: &Connection, t: &Taxonomy) -> Result<(), StorageError> {
    t.validate()?;
    // `merged_into` may point at a category saved later in the loop; check at commit.
    conn.execute_batch("PRAGMA defer_foreign_keys = ON;")?;
    conn.execute(
        "INSERT INTO taxonomy (id, base, revision) VALUES (1, ?1, ?2)
         ON CONFLICT (id) DO UPDATE SET base = excluded.base, revision = excluded.revision",
        params![t.version.base, t.version.revision],
    )?;
    // Keys are unique, so a save that swaps keys would collide midway: park every
    // existing key on a placeholder first (char(1) never appears in a real key).
    conn.execute_batch(
        "UPDATE categories SET key = char(1) || id;
         UPDATE category_groups SET key = char(1) || id;",
    )?;
    for g in &t.groups {
        conn.execute(
            "INSERT INTO category_groups (id, key, name, hidden) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (id) DO UPDATE SET key = excluded.key, name = excluded.name,
                                            hidden = excluded.hidden",
            params![g.id.0, g.key, g.name, g.hidden],
        )?;
    }
    for c in &t.categories {
        conn.execute(
            "INSERT INTO categories (id, group_id, key, name, hidden, merged_into)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (id) DO UPDATE SET group_id = excluded.group_id, key = excluded.key,
                 name = excluded.name, hidden = excluded.hidden, merged_into = excluded.merged_into",
            params![c.id.0, c.group_id.0, c.key, c.name, c.hidden, c.merged_into.map(|m| m.0)],
        )?;
    }
    // Rows not in the new list go. Removing a category that decisions or rules still
    // use fails (RESTRICT): merge it instead.
    let ids = |ids: Vec<i64>| serde_json::to_string(&ids).expect("integers serialise");
    conn.execute(
        "DELETE FROM categories WHERE id NOT IN (SELECT value FROM json_each(?1))",
        [ids(t.categories.iter().map(|c| c.id.0).collect())],
    )?;
    conn.execute(
        "DELETE FROM category_groups WHERE id NOT IN (SELECT value FROM json_each(?1))",
        [ids(t.groups.iter().map(|g| g.id.0).collect())],
    )?;
    Ok(())
}

fn load_taxonomy(conn: &Connection) -> Result<Option<Taxonomy>, StorageError> {
    let Some(version) = conn
        .query_row(
            "SELECT base, revision FROM taxonomy WHERE id = 1",
            [],
            |r| {
                Ok(TaxonomyVersion {
                    base: r.get(0)?,
                    revision: r.get(1)?,
                })
            },
        )
        .optional()?
    else {
        return Ok(None);
    };
    let groups = conn
        .prepare("SELECT id, key, name, hidden FROM category_groups ORDER BY id")?
        .query_map([], |r| {
            Ok(CategoryGroup {
                id: CategoryGroupId(r.get(0)?),
                key: r.get(1)?,
                name: r.get(2)?,
                hidden: r.get(3)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    let categories = conn
        .prepare("SELECT id, group_id, key, name, hidden, merged_into FROM categories ORDER BY id")?
        .query_map([], |r| {
            Ok(Category {
                id: CategoryId(r.get(0)?),
                group_id: CategoryGroupId(r.get(1)?),
                key: r.get(2)?,
                name: r.get(3)?,
                hidden: r.get(4)?,
                merged_into: r.get::<_, Option<i64>>(5)?.map(CategoryId),
            })
        })?
        .collect::<Result<_, _>>()?;
    let taxonomy = Taxonomy {
        version,
        groups,
        categories,
    };
    taxonomy
        .validate()
        .map_err(|_| invalid("categories", "category list breaks its invariants"))?;
    Ok(Some(taxonomy))
}

/// Stores the default category list if the ledger has none yet. Runs on every open, so
/// new ledgers and ones created before migration 2 both get it.
pub(crate) fn seed_default_taxonomy(conn: &mut Connection) -> Result<(), StorageError> {
    let tx = conn.transaction()?;
    if load_taxonomy(&tx)?.is_none() {
        save_taxonomy(&tx, &default_taxonomy())?;
    }
    tx.commit()?;
    Ok(())
}

// ---- Rules

const RULE_COLUMNS: &str = "id, match_kind, match_text, account_id, amount_kind, \
     amount_min_minor, amount_max_minor, amount_currency, bucket, bucket_category_id, \
     bucket_transaction_id, merchant, origin, origin_transaction_id, origin_decision_id, \
     priority, enabled, created_at";

fn insert_rule(conn: &Connection, rule: &Rule) -> Result<(), StorageError> {
    rule.validate()?;
    let (match_kind, match_text) = match &rule.matcher.description {
        TextMatch::Equals(s) => ("equals", s),
        TextMatch::StartsWith(s) => ("starts_with", s),
        TextMatch::Contains(s) => ("contains", s),
    };
    let (amount_kind, min, max, amount_currency) = match rule.matcher.amount {
        None => (None, None, None, None),
        Some(AmountMatch::Exact(m)) => (
            Some("exact"),
            Some(m.minor()),
            None,
            Some(m.currency().code()),
        ),
        Some(AmountMatch::Between { min, max }) => (
            Some("between"),
            Some(min.minor()),
            Some(max.minor()),
            Some(min.currency().code()),
        ),
    };
    let (bucket, bucket_category, bucket_transaction) = match &rule.action.bucket {
        Some(b) => {
            let (k, c, t) = bucket_columns(b);
            (Some(k), c, t)
        }
        None => (None, None, None),
    };
    let (origin, origin_transaction, origin_decision) = match rule.origin {
        RuleOrigin::User => ("user", None, None),
        RuleOrigin::Correction {
            transaction_id,
            decision_id,
        } => ("correction", Some(transaction_id.0), Some(decision_id.0)),
    };
    conn.prepare(&format!(
        "INSERT INTO rules ({RULE_COLUMNS}) VALUES \
         (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)"
    ))?
    .execute(params![
        rule.id.0,
        match_kind,
        match_text,
        rule.matcher.account_id.map(|a| a.0),
        amount_kind,
        min,
        max,
        amount_currency,
        bucket,
        bucket_category,
        bucket_transaction,
        rule.action.merchant,
        origin,
        origin_transaction,
        origin_decision,
        rule.priority,
        rule.enabled,
        rule.created_at.unix_seconds(),
    ])?;
    Ok(())
}

fn rule_from_row(r: &Row<'_>) -> Result<Rule, StorageError> {
    const T: &str = "rules";
    let text: String = r.get(2)?;
    let description = match r.get::<_, String>(1)?.as_str() {
        "equals" => TextMatch::Equals(text),
        "starts_with" => TextMatch::StartsWith(text),
        "contains" => TextMatch::Contains(text),
        _ => return Err(invalid(T, "unknown match kind")),
    };
    let amount_currency = r
        .get::<_, Option<String>>(7)?
        .map(|c| currency(T, c))
        .transpose()?;
    let amount = match (r.get::<_, Option<String>>(4)?.as_deref(), amount_currency) {
        (None, _) => None,
        (Some("exact"), Some(c)) => Some(AmountMatch::Exact(Money::new(r.get(5)?, c))),
        (Some("between"), Some(c)) => Some(AmountMatch::Between {
            min: Money::new(r.get(5)?, c),
            max: Money::new(r.get(6)?, c),
        }),
        _ => return Err(invalid(T, "malformed amount match")),
    };
    let bucket = match r.get::<_, Option<String>>(8)? {
        Some(kind) => Some(bucket_from_columns(T, &kind, r.get(9)?, r.get(10)?)?),
        None => None,
    };
    let origin = match r.get::<_, String>(12)?.as_str() {
        "user" => RuleOrigin::User,
        "correction" => RuleOrigin::Correction {
            transaction_id: TransactionId(r.get(13)?),
            decision_id: DecisionId(r.get(14)?),
        },
        _ => return Err(invalid(T, "unknown rule origin")),
    };
    let rule = Rule {
        id: RuleId(r.get(0)?),
        matcher: RuleMatcher {
            description,
            account_id: r.get::<_, Option<i64>>(3)?.map(AccountId),
            amount,
        },
        action: RuleAction {
            bucket,
            merchant: r.get(11)?,
        },
        origin,
        priority: r.get(15)?,
        enabled: r.get(16)?,
        created_at: Timestamp::from_unix_seconds(r.get(17)?),
    };
    rule.validate()
        .map_err(|_| invalid(T, "rule breaks its invariants"))?;
    Ok(rule)
}

// ---- Decisions

const DECISION_COLUMNS: &str = "id, transaction_id, assignment, bucket, bucket_category_id, \
     bucket_transaction_id, merchant, source, rule_id, builtin_logic, builtin_version, model_id, \
     model_gguf_sha256, prompt_version, confidence_bp, taxonomy_base, taxonomy_revision, \
     decided_at, state, seen_at, replaces";

/// The `model_*` columns of a decision (all set for a model decision, else all NULL).
#[derive(Default)]
struct ModelColumns<'a> {
    model_id: Option<&'a str>,
    gguf_sha256: Option<&'a [u8; 32]>,
    prompt_version: Option<&'a str>,
    confidence_bp: Option<u16>,
}

fn insert_decision(conn: &Connection, d: &Decision) -> Result<(), StorageError> {
    let (assignment, bucket, bucket_category, bucket_transaction, merchant) = match &d.assignment {
        Assignment::Bucket(b) => {
            let (k, c, t) = bucket_columns(b);
            ("bucket", Some(k), c, t, None)
        }
        Assignment::Merchant(name) => ("merchant", None, None, None, Some(name.as_str())),
    };
    let mut rule_id = None;
    let mut builtin: (Option<String>, Option<u32>) = (None, None);
    let mut model = ModelColumns::default();
    let source = match &d.source {
        DecisionSource::User => "user",
        DecisionSource::Rule { rule_id: id } => {
            rule_id = Some(id.0);
            "rule"
        }
        DecisionSource::Builtin { logic, version } => {
            builtin = (Some(enum_text(logic)), Some(*version));
            "builtin"
        }
        DecisionSource::Model(p) => {
            model = ModelColumns {
                model_id: Some(&p.model_id),
                gguf_sha256: Some(p.gguf_sha256.as_bytes()),
                prompt_version: Some(&p.prompt_version),
                confidence_bp: Some(p.confidence.basis_points()),
            };
            "model"
        }
    };
    conn.prepare(&format!(
        "INSERT INTO decisions ({DECISION_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, \
         ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)"
    ))?
    .execute(params![
        d.id.0,
        d.transaction_id.0,
        assignment,
        bucket,
        bucket_category,
        bucket_transaction,
        merchant,
        source,
        rule_id,
        builtin.0,
        builtin.1,
        model.model_id,
        model.gguf_sha256.map(<[u8; 32]>::as_slice),
        model.prompt_version,
        model.confidence_bp,
        d.taxonomy_version.base,
        d.taxonomy_version.revision,
        d.decided_at.unix_seconds(),
        enum_text(&d.state),
        d.seen_at.map(Timestamp::unix_seconds),
        d.replaces.map(|r| r.0),
    ])
    .map_err(map_write_error)?;
    Ok(())
}

fn decision_from_row(r: &Row<'_>) -> Result<Decision, StorageError> {
    const T: &str = "decisions";
    let assignment = match r.get::<_, String>(2)?.as_str() {
        "bucket" => {
            let kind: String = r.get(3)?;
            Assignment::Bucket(bucket_from_columns(T, &kind, r.get(4)?, r.get(5)?)?)
        }
        "merchant" => Assignment::Merchant(r.get(6)?),
        _ => return Err(invalid(T, "unknown assignment")),
    };
    let source = match r.get::<_, String>(7)?.as_str() {
        "user" => DecisionSource::User,
        "rule" => DecisionSource::Rule {
            rule_id: RuleId(r.get(8)?),
        },
        "builtin" => DecisionSource::Builtin {
            logic: text_enum::<BuiltinLogic>(T, r.get(9)?)?,
            version: r.get(10)?,
        },
        "model" => DecisionSource::Model(ModelProvenance {
            model_id: r.get(11)?,
            gguf_sha256: hash(T, r.get(12)?)?,
            prompt_version: r.get(13)?,
            confidence: Confidence::from_basis_points(r.get(14)?)
                .map_err(|_| invalid(T, "confidence out of range"))?,
        }),
        _ => return Err(invalid(T, "unknown source")),
    };
    Ok(Decision {
        id: DecisionId(r.get(0)?),
        transaction_id: TransactionId(r.get(1)?),
        assignment,
        source,
        taxonomy_version: TaxonomyVersion {
            base: r.get(15)?,
            revision: r.get(16)?,
        },
        decided_at: Timestamp::from_unix_seconds(r.get(17)?),
        state: text_enum(T, r.get(18)?)?,
        seen_at: r
            .get::<_, Option<i64>>(19)?
            .map(Timestamp::from_unix_seconds),
        replaces: r.get::<_, Option<i64>>(20)?.map(DecisionId),
    })
}

// ---- Generic reads

fn query_all<T>(
    conn: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
    decode: fn(&Row<'_>) -> Result<T, StorageError>,
) -> Result<Vec<T>, StorageError> {
    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query(params)?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(decode(row)?);
    }
    Ok(out)
}

fn query_one<T>(
    conn: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
    decode: fn(&Row<'_>) -> Result<T, StorageError>,
) -> Result<Option<T>, StorageError> {
    let mut all = query_all(conn, sql, params, decode)?;
    Ok(all.pop())
}

/// Deletes the transactions in `scope`, first recording the ones whose import isn't in
/// `kept_out` (the imports the plan deletes next) in `deleted_rows`. Returns how many
/// rows it recorded. Runs under the delete permit.
fn delete_scope(
    tx: &Connection,
    scope: &DeletionScope,
    kept_out: &[ImportId],
) -> Result<u64, StorageError> {
    const IN_SCOPE: &str = "(?1 IS NULL OR account_id = ?1) \
         AND (?2 IS NULL OR coalesce(transaction_date, posted_date) >= ?2) \
         AND (?3 IS NULL OR coalesce(transaction_date, posted_date) <= ?3)";
    let (account, from, to) = (
        scope.account.map(|a| a.0),
        scope.from.map(date_text),
        scope.to.map(date_text),
    );
    let scope_params = params![account, from, to];
    let mut rows = Vec::new();
    {
        let mut statement = tx.prepare(&format!(
            "SELECT import_id, source_index FROM transactions WHERE {IN_SCOPE}"
        ))?;
        let mut found = statement.query(scope_params)?;
        while let Some(r) = found.next()? {
            let import: i64 = r.get(0)?;
            if !kept_out.contains(&ImportId(import)) {
                rows.push((import, r.get::<_, i64>(1)?));
            }
        }
    }
    let mut recorded = 0;
    {
        // OR IGNORE: a row recorded already stays recorded.
        let mut insert = tx.prepare(
            "INSERT OR IGNORE INTO deleted_rows (import_id, source_index) VALUES (?1, ?2)",
        )?;
        for (import, index) in rows {
            recorded += insert.execute([import, index])? as u64;
        }
    }
    tx.execute(
        &format!("DELETE FROM transactions WHERE {IN_SCOPE}"),
        scope_params,
    )?;
    Ok(recorded)
}

fn count(conn: &Connection, table: &str) -> Result<u64, StorageError> {
    let n: i64 = conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))?;
    Ok(n.unsigned_abs())
}

/// The public repository API, on an unlocked ledger.
impl Unlocked {
    /// An id never used in `table`: one more than the largest it has ever held, so a
    /// deleted row's id is not given to a new one (migration 6, `id_marks`).
    pub fn next_id(&self, table: Table) -> Result<i64, StorageError> {
        let name = table.name();
        Ok(self.connection().query_row(
            &format!(
                "SELECT max(coalesce((SELECT max(id) FROM {name}), 0),
                            coalesce((SELECT high FROM id_marks WHERE table_name = ?1), 0)) + 1"
            ),
            [name],
            |r| r.get(0),
        )?)
    }

    pub fn insert_account(&self, account: &Account) -> Result<(), StorageError> {
        insert_account(self.connection(), account)
    }

    pub fn account(&self, id: AccountId) -> Result<Option<Account>, StorageError> {
        let sql = format!("SELECT {ACCOUNT_COLUMNS} FROM accounts WHERE id = ?1");
        query_one(self.connection(), &sql, [id.0], account_from_row)
    }

    pub fn accounts(&self) -> Result<Vec<Account>, StorageError> {
        let sql = format!("SELECT {ACCOUNT_COLUMNS} FROM accounts ORDER BY id");
        query_all(self.connection(), &sql, [], account_from_row)
    }

    pub fn insert_raw_import(&self, raw: &RawImport) -> Result<(), StorageError> {
        insert_raw_import(self.connection(), raw)
    }

    pub fn raw_import(&self, id: ImportId) -> Result<Option<RawImport>, StorageError> {
        let sql = format!("SELECT {RAW_IMPORT_COLUMNS} FROM raw_imports WHERE id = ?1");
        query_one(self.connection(), &sql, [id.0], raw_import_from_row)
    }

    pub fn raw_imports(&self) -> Result<Vec<RawImport>, StorageError> {
        let sql = format!("SELECT {RAW_IMPORT_COLUMNS} FROM raw_imports ORDER BY id");
        query_all(self.connection(), &sql, [], raw_import_from_row)
    }

    /// Every import's source and CSV mapping profile, without its bytes: what the
    /// pipeline needs to know which dates an earlier import's rows carry.
    pub(crate) fn import_origins(
        &self,
    ) -> Result<
        std::collections::HashMap<ImportId, (crate::model::ImportSource, Option<String>)>,
        StorageError,
    > {
        const T: &str = "raw_imports";
        let mut stmt = self
            .connection()
            .prepare("SELECT id, source, mapping_profile FROM raw_imports")?;
        let mut rows = stmt.query([])?;
        let mut out = std::collections::HashMap::new();
        while let Some(r) = rows.next()? {
            out.insert(
                ImportId(r.get(0)?),
                (text_enum(T, r.get(1)?)?, r.get::<_, Option<String>>(2)?),
            );
        }
        Ok(out)
    }

    /// Imports whose bytes have this SHA-256 (the same file imported before).
    pub fn imports_with_sha256(&self, sha256: Sha256Hash) -> Result<Vec<ImportId>, StorageError> {
        let mut stmt = self
            .connection()
            .prepare("SELECT id FROM raw_imports WHERE sha256 = ?1 ORDER BY id")?;
        let ids = stmt
            .query_map([sha256.as_bytes().as_slice()], |r| r.get(0).map(ImportId))?
            .collect::<Result<_, _>>()?;
        Ok(ids)
    }

    pub fn insert_transaction(&self, transaction: &Transaction) -> Result<(), StorageError> {
        insert_transaction(self.connection(), transaction)
    }

    pub fn transaction(&self, id: TransactionId) -> Result<Option<Transaction>, StorageError> {
        let sql = format!("SELECT {TRANSACTION_COLUMNS} FROM transactions WHERE id = ?1");
        query_one(self.connection(), &sql, [id.0], transaction_from_row)
    }

    pub fn transactions(&self) -> Result<Vec<Transaction>, StorageError> {
        let sql = format!("SELECT {TRANSACTION_COLUMNS} FROM transactions ORDER BY id");
        query_all(self.connection(), &sql, [], transaction_from_row)
    }

    pub fn transactions_for_account(
        &self,
        account: AccountId,
    ) -> Result<Vec<Transaction>, StorageError> {
        let sql = format!(
            "SELECT {TRANSACTION_COLUMNS} FROM transactions WHERE account_id = ?1 \
             ORDER BY posted_date, id"
        );
        query_all(self.connection(), &sql, [account.0], transaction_from_row)
    }

    /// The category list, as stored. Every ledger has one (the default until edited).
    pub fn taxonomy(&self) -> Result<Taxonomy, StorageError> {
        load_taxonomy(self.connection())?.ok_or(invalid("taxonomy", "no category list stored"))
    }

    /// Replaces the category list with `taxonomy` (validated first). Ids are kept, so
    /// renames re-apply to past decisions. A category that decisions or rules still
    /// use can't be removed: merge it instead.
    pub fn save_taxonomy(&mut self, taxonomy: &Taxonomy) -> Result<(), StorageError> {
        let tx = self.connection_mut().transaction()?;
        save_taxonomy(&tx, taxonomy)?;
        tx.commit()?;
        Ok(())
    }

    pub fn insert_rule(&self, rule: &Rule) -> Result<(), StorageError> {
        insert_rule(self.connection(), rule)
    }

    pub fn rules(&self) -> Result<Vec<Rule>, StorageError> {
        let sql = format!("SELECT {RULE_COLUMNS} FROM rules ORDER BY id");
        query_all(self.connection(), &sql, [], rule_from_row)
    }

    /// Turns a rule on or off. Allowed even once the rule has made decisions: only what
    /// it matches and does is frozen. What it decided stays; see
    /// `categorize::undo_rule_decisions`.
    pub fn set_rule_enabled(&self, id: RuleId, enabled: bool) -> Result<(), StorageError> {
        let changed = self
            .connection()
            .execute(
                "UPDATE rules SET enabled = ?2 WHERE id = ?1",
                params![id.0, i64::from(enabled)],
            )
            .map_err(map_write_error)?;
        if changed == 0 {
            return Err(StorageError::NotFound);
        }
        Ok(())
    }

    /// Deletes a rule and, by the cascade, every decision it made (any state). A
    /// decision that `replaces` one of them keeps standing with `replaces` cleared.
    /// Returns how many decisions went. Transactions whose bucket came from the rule are
    /// left without one: re-plan them in the same transaction (`categorize::delete_rule`
    /// does).
    pub fn delete_rule(&self, id: RuleId) -> Result<u64, StorageError> {
        let conn = self.connection();
        let before = count(conn, "decisions")?;
        let deleted = {
            let _delete = self.permits.grant(Permit::Delete);
            conn.execute("DELETE FROM rules WHERE id = ?1", [id.0])
                .map_err(map_write_error)?
        };
        if deleted == 0 {
            return Err(StorageError::NotFound);
        }
        Ok(before - count(conn, "decisions")?)
    }

    /// Records a decision. Decisions are append-only: once stored, only
    /// [`Unlocked::set_decision_state`] and [`Unlocked::mark_decision_seen`] change them.
    pub fn insert_decision(&self, decision: &Decision) -> Result<(), StorageError> {
        insert_decision(self.connection(), decision)
    }

    pub fn decision(&self, id: DecisionId) -> Result<Option<Decision>, StorageError> {
        let sql = format!("SELECT {DECISION_COLUMNS} FROM decisions WHERE id = ?1");
        query_one(self.connection(), &sql, [id.0], decision_from_row)
    }

    pub fn decisions(&self) -> Result<Vec<Decision>, StorageError> {
        let sql = format!("SELECT {DECISION_COLUMNS} FROM decisions ORDER BY id");
        query_all(self.connection(), &sql, [], decision_from_row)
    }

    /// Every decision about one transaction, oldest first.
    pub fn decisions_for_transaction(
        &self,
        transaction: TransactionId,
    ) -> Result<Vec<Decision>, StorageError> {
        let sql = format!(
            "SELECT {DECISION_COLUMNS} FROM decisions WHERE transaction_id = ?1 ORDER BY id"
        );
        query_all(self.connection(), &sql, [transaction.0], decision_from_row)
    }

    /// Moves a decision along its lifecycle:
    /// - applied → superseded (another decision replaced it);
    /// - applied → dismissed, only once the user has seen it ([`mark_decision_seen`]):
    ///   the user rejected it (SPEC decision 46). The caller records what replaces it,
    ///   so the transaction keeps one applied bucket (constraint 4);
    /// - suggested → applied, dismissed or superseded.
    ///
    /// Anything else, including "moving" to the state it is already in or dismissing an
    /// applied decision nobody has seen, is [`StorageError::ForbiddenChange`]. The
    /// database refuses it too.
    ///
    /// [`mark_decision_seen`]: Self::mark_decision_seen
    pub fn set_decision_state(
        &self,
        id: DecisionId,
        state: DecisionState,
    ) -> Result<(), StorageError> {
        let changed = self
            .connection()
            .execute(
                "UPDATE decisions SET state = ?2 WHERE id = ?1",
                params![id.0, enum_text(&state)],
            )
            .map_err(map_write_error)?;
        if changed == 0 {
            return Err(StorageError::NotFound);
        }
        Ok(())
    }

    /// Records when the user first saw a decision. Setting it again, even to the same
    /// time, is [`StorageError::ForbiddenChange`]: the first time is what counts
    /// (constraint 7).
    pub fn mark_decision_seen(&self, id: DecisionId, at: Timestamp) -> Result<(), StorageError> {
        let changed = self
            .connection()
            .execute(
                "UPDATE decisions SET seen_at = ?2 WHERE id = ?1",
                params![id.0, at.unix_seconds()],
            )
            .map_err(map_write_error)?;
        if changed == 0 {
            return Err(StorageError::NotFound);
        }
        Ok(())
    }

    /// Turns a pending transaction into the posted one (T1.3): `posted` replaces every
    /// field of the stored transaction with the same id, except the account, and must be
    /// [`TransactionStatus::Posted`](crate::model::TransactionStatus). The posted record
    /// may come from a newer import, so `source` can point elsewhere; the transaction then
    /// goes with that import if it is deleted. This is the only change a stored
    /// transaction can take: anything else is [`StorageError::ForbiddenChange`].
    /// Decisions already made about the pending transaction stay; deciding again is up
    /// to the caller.
    pub fn post_pending(&self, posted: &Transaction) -> Result<(), StorageError> {
        let changed = {
            let _post = self.permits.grant(Permit::PostPending);
            self.connection()
                .execute(
                    "UPDATE transactions SET import_id = ?2, source_index = ?3, external_id = ?4,
                         status = ?5, posted_date = ?6, transaction_date = ?7, amount_minor = ?8,
                         currency = ?9, original_amount_minor = ?10, original_currency = ?11,
                         description = ?12, payee = ?13, memo = ?14, trn_type = ?16,
                         check_number = ?17
                     WHERE id = ?1 AND account_id = ?15",
                    params![
                        posted.id.0,
                        posted.source.import_id.0,
                        posted.source.index,
                        posted.source.external_id,
                        enum_text(&posted.status),
                        date_text(posted.posted_date),
                        posted.transaction_date.map(date_text),
                        posted.amount.minor(),
                        posted.amount.currency().code(),
                        posted.original_amount.map(Money::minor),
                        posted.original_amount.map(|m| m.currency().code()),
                        posted.description,
                        posted.payee,
                        posted.memo,
                        posted.account_id.0,
                        posted.trn_type,
                        posted.check_number,
                    ],
                )
                .map_err(map_write_error)?
        };
        if changed == 0 {
            return Err(StorageError::NotFound);
        }
        Ok(())
    }

    /// Deletes an import and everything derived from it (SPEC decision 28): its
    /// transactions; decisions about them or pointing at them (transfer counterparts,
    /// refund originals); rules learned from them; and the decisions those rules made.
    /// A surviving decision that `replaces` a deleted one keeps standing, with
    /// `replaces` cleared. Runs in one transaction.
    pub fn delete_import(&mut self, id: ImportId) -> Result<DeletedCounts, StorageError> {
        self.delete_planned(
            &DeletionPlan {
                imports: vec![id],
                ..DeletionPlan::default()
            },
            false,
        )
    }

    /// Carries out `plan` in one transaction, in its order (see [`DeletionPlan`]), each
    /// part with everything derived from it as for [`delete_import`](Self::delete_import).
    /// Accounts delete the rules scoped to them, with those rules' decisions.
    ///
    /// A transaction removed by the scope whose import is kept is recorded in
    /// `deleted_rows` (SPEC decision 43, see [`deleted_rows`](Self::deleted_rows)).
    ///
    /// With `dry_run`, everything is done and then rolled back: the counts are exactly
    /// what the real run would remove, and the ledger is left as it was.
    ///
    /// Refuses an empty plan, an inverted date range and an id listed twice; fails with
    /// [`StorageError::NotFound`] for an import or account that doesn't exist and with
    /// [`StorageError::AccountInUse`] for an account that still has transactions or
    /// imports after the earlier parts. Any failure leaves the ledger unchanged.
    pub(crate) fn delete_planned(
        &mut self,
        plan: &DeletionPlan,
        dry_run: bool,
    ) -> Result<DeletedCounts, StorageError> {
        plan.check()?;
        let permits = std::sync::Arc::clone(&self.permits);
        let tx = self.connection_mut().transaction()?;
        let before = table_counts(&tx)?;
        let mut recorded_rows = 0;
        {
            let _delete = permits.grant(Permit::Delete);
            if let Some(scope) = &plan.transactions {
                recorded_rows = delete_scope(&tx, scope, &plan.imports)?;
            }
            for id in &plan.imports {
                if tx.execute("DELETE FROM raw_imports WHERE id = ?1", [id.0])? == 0 {
                    return Err(StorageError::NotFound);
                }
            }
            for id in &plan.accounts {
                let in_use: bool = tx.query_row(
                    "SELECT EXISTS (SELECT 1 FROM transactions WHERE account_id = ?1) \
                         OR EXISTS (SELECT 1 FROM raw_imports WHERE account_id = ?1)",
                    [id.0],
                    |r| r.get(0),
                )?;
                if in_use {
                    return Err(StorageError::AccountInUse);
                }
                if tx.execute("DELETE FROM accounts WHERE id = ?1", [id.0])? == 0 {
                    return Err(StorageError::NotFound);
                }
            }
        }
        let after = table_counts(&tx)?;
        let counts = DeletedCounts {
            transactions: before.transactions - after.transactions,
            decisions: before.decisions - after.decisions,
            rules: before.rules - after.rules,
            imports: before.imports - after.imports,
            accounts: before.accounts - after.accounts,
            recorded_rows,
        };
        if dry_run {
            tx.rollback()?;
        } else {
            tx.commit()?;
        }
        Ok(counts)
    }

    /// The rows of import `id` deleted while the import was kept (SPEC decision 43), as
    /// source indexes in ascending order. Anything that re-reads the import's raw bytes
    /// must skip these rows, so a deleted transaction never comes back from its file.
    #[allow(
        dead_code,
        reason = "read by T5.3 and by any re-parse of a raw import (SPEC decision 43)"
    )]
    pub(crate) fn deleted_rows(&self, id: ImportId) -> Result<Vec<u32>, StorageError> {
        let mut statement = self.connection().prepare(
            "SELECT source_index FROM deleted_rows WHERE import_id = ?1 ORDER BY source_index",
        )?;
        let rows = statement.query_map([id.0], |r| r.get::<_, i64>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(u32::try_from(row?).map_err(|_| StorageError::InvalidRow {
                table: "deleted_rows",
                reason: "source index out of range",
            })?);
        }
        Ok(out)
    }

    /// Writes a whole ledger into this (empty) one, in one transaction: the T0.5
    /// fixtures and TD.1 ground truth go through here. The taxonomy replaces the
    /// current one. References between decisions and rules may point either way;
    /// foreign keys are checked at commit.
    pub fn save_ledger(&mut self, ledger: &Ledger) -> Result<(), StorageError> {
        let tx = self.connection_mut().transaction()?;
        for table in [
            "accounts",
            "raw_imports",
            "transactions",
            "decisions",
            "rules",
        ] {
            if count(&tx, table)? != 0 {
                return Err(StorageError::AlreadyExists);
            }
        }
        tx.execute_batch("PRAGMA defer_foreign_keys = ON;")?;
        save_taxonomy(&tx, &ledger.taxonomy)?;
        for a in &ledger.accounts {
            insert_account(&tx, a)?;
        }
        for r in &ledger.imports {
            insert_raw_import(&tx, r)?;
        }
        for t in &ledger.transactions {
            insert_transaction(&tx, t)?;
        }
        for r in &ledger.rules {
            insert_rule(&tx, r)?;
        }
        for d in &ledger.decisions {
            insert_decision(&tx, d)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Reads the whole ledger, every table in id order.
    pub fn load_ledger(&self) -> Result<Ledger, StorageError> {
        Ok(Ledger {
            accounts: self.accounts()?,
            imports: self.raw_imports()?,
            transactions: self.transactions()?,
            taxonomy: self.taxonomy()?,
            decisions: self.decisions()?,
            rules: self.rules()?,
        })
    }
}

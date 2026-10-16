-- Migration 2: the ledger tables for the T0.5 data model (crates/fm-core/src/model).
-- Column meanings and units are in crates/fm-core/src/model/README.md; the storage
-- rules (cascades, append-only decisions) are in crates/fm-core/src/storage/README.md.
-- Dates are ISO 'YYYY-MM-DD' text, timestamps are Unix seconds, amounts are integer
-- minor units next to an ISO 4217 code, booleans are 0/1.
--
-- fm_permit(kind) is an SQL function the app registers on each connection (db.rs). It
-- returns 1 only while the matching repository API call is running in Rust: 'delete'
-- during delete_import, 'post_pending' during post_pending. No SQL statement can turn
-- it on, and SQL can't define functions. A connection without it (another tool opening
-- the file with the key) can't change or delete protected rows at all: the triggers
-- below fail closed. Connections run with recursive_triggers on, so REPLACE's implicit
-- delete goes through the DELETE triggers too.

CREATE TABLE accounts (
    id            INTEGER PRIMARY KEY,
    name          TEXT    NOT NULL,
    kind          TEXT    NOT NULL CHECK (kind IN ('checking', 'savings', 'credit_card', 'cash',
                                                  'loan', 'mortgage', 'brokerage', 'retirement', 'other')),
    currency      TEXT    NOT NULL CHECK (length(currency) = 3),
    institution   TEXT,
    masked_number TEXT,
    external_id   TEXT,
    own_account   INTEGER NOT NULL CHECK (own_account IN (0, 1)),
    created_at    INTEGER NOT NULL
) STRICT;

-- An account's id and currency never change: its transactions' amounts are in it.
CREATE TRIGGER accounts_keep_id_and_currency
BEFORE UPDATE OF id, currency ON accounts
WHEN NEW.id IS NOT OLD.id OR NEW.currency IS NOT OLD.currency
BEGIN
    SELECT RAISE(ABORT, 'immutable: an account keeps its id and currency');
END;

CREATE TABLE raw_imports (
    id              INTEGER PRIMARY KEY,
    source          TEXT    NOT NULL CHECK (source IN ('csv', 'ofx', 'qfx', 'simplefin', 'manual')),
    file_name       TEXT,
    mapping_profile TEXT,
    account_id      INTEGER REFERENCES accounts (id) ON DELETE RESTRICT,
    imported_at     INTEGER NOT NULL,
    sha256          BLOB    NOT NULL CHECK (length(sha256) = 32),
    bytes           BLOB    NOT NULL
) STRICT;
CREATE INDEX raw_imports_by_sha256 ON raw_imports (sha256);

-- The raw layer is never rewritten (SPEC decision 28: keep exactly what came in), and
-- an import goes only through delete_import, which takes everything derived with it.
CREATE TRIGGER raw_imports_are_immutable
BEFORE UPDATE OF id, sha256, bytes ON raw_imports
WHEN NEW.id IS NOT OLD.id OR NEW.sha256 IS NOT OLD.sha256 OR NEW.bytes IS NOT OLD.bytes
BEGIN
    SELECT RAISE(ABORT, 'immutable: raw import id and bytes never change');
END;

CREATE TRIGGER raw_imports_are_deleted_only_through_the_api
BEFORE DELETE ON raw_imports
WHEN NOT fm_permit('delete')
BEGIN
    SELECT RAISE(ABORT, 'immutable: raw imports are removed only by delete_import');
END;

CREATE TABLE transactions (
    id                    INTEGER PRIMARY KEY,
    account_id            INTEGER NOT NULL REFERENCES accounts (id) ON DELETE RESTRICT,
    import_id             INTEGER NOT NULL REFERENCES raw_imports (id) ON DELETE CASCADE,
    source_index          INTEGER NOT NULL CHECK (source_index BETWEEN 0 AND 4294967295),
    external_id           TEXT,
    status                TEXT    NOT NULL CHECK (status IN ('pending', 'posted')),
    posted_date           TEXT    NOT NULL CHECK (posted_date GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]'),
    transaction_date      TEXT             CHECK (transaction_date GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]'),
    amount_minor          INTEGER NOT NULL,
    currency              TEXT    NOT NULL CHECK (length(currency) = 3),
    original_amount_minor INTEGER,
    original_currency     TEXT             CHECK (length(original_currency) = 3),
    description           TEXT    NOT NULL,
    payee                 TEXT,
    memo                  TEXT,
    CHECK ((original_amount_minor IS NULL) = (original_currency IS NULL)),
    UNIQUE (import_id, source_index)
) STRICT;
CREATE INDEX transactions_by_account_date ON transactions (account_id, posted_date);
CREATE INDEX transactions_by_external_id ON transactions (external_id) WHERE external_id IS NOT NULL;

-- A transaction records what the source said, so it never changes, with one exception:
-- the import pipeline (T1.3) turns a pending transaction into the posted one through
-- post_pending, which may update everything except the id and the account (the posted
-- record can come from a newer import, with a different amount, date or description).
CREATE TRIGGER transactions_are_immutable
BEFORE UPDATE ON transactions
WHEN NOT (fm_permit('post_pending') AND OLD.status = 'pending' AND NEW.status = 'posted'
          AND NEW.id IS OLD.id AND NEW.account_id IS OLD.account_id)
BEGIN
    SELECT RAISE(ABORT, 'immutable: only a pending transaction may become posted');
END;

CREATE TRIGGER transactions_are_deleted_only_with_their_import
BEFORE DELETE ON transactions
WHEN NOT fm_permit('delete')
BEGIN
    SELECT RAISE(ABORT, 'immutable: transactions are removed only with their import');
END;

-- One row: the version of the category list in force.
CREATE TABLE taxonomy (
    id       INTEGER PRIMARY KEY CHECK (id = 1),
    base     TEXT    NOT NULL,
    revision INTEGER NOT NULL CHECK (revision >= 0)
) STRICT;

CREATE TABLE category_groups (
    id     INTEGER PRIMARY KEY,
    key    TEXT    NOT NULL UNIQUE,
    name   TEXT    NOT NULL,
    hidden INTEGER NOT NULL CHECK (hidden IN (0, 1))
) STRICT;

CREATE TABLE categories (
    id          INTEGER PRIMARY KEY,
    group_id    INTEGER NOT NULL REFERENCES category_groups (id) ON DELETE RESTRICT,
    key         TEXT    NOT NULL UNIQUE,
    name        TEXT    NOT NULL,
    hidden      INTEGER NOT NULL CHECK (hidden IN (0, 1)),
    merged_into INTEGER REFERENCES categories (id) ON DELETE RESTRICT
) STRICT;

-- A Bucket is stored as (bucket, bucket_category_id, bucket_transaction_id):
--   category     → category id,  no transaction
--   transfer     → no category,  counterpart (optional)
--   income       → neither
--   refund       → category id,  original purchase (optional)
--   needs_review → neither
-- Used by decisions (assignment = 'bucket') and by rule actions.

CREATE TABLE rules (
    id                    INTEGER PRIMARY KEY,
    match_kind            TEXT    NOT NULL CHECK (match_kind IN ('equals', 'starts_with', 'contains')),
    match_text            TEXT    NOT NULL,
    account_id            INTEGER REFERENCES accounts (id) ON DELETE CASCADE,
    amount_kind           TEXT    CHECK (amount_kind IN ('exact', 'between')),
    amount_min_minor      INTEGER,
    amount_max_minor      INTEGER,
    amount_currency       TEXT    CHECK (length(amount_currency) = 3),
    bucket                TEXT    CHECK (bucket IN ('category', 'transfer', 'income', 'refund', 'needs_review')),
    bucket_category_id    INTEGER REFERENCES categories (id) ON DELETE RESTRICT,
    bucket_transaction_id INTEGER REFERENCES transactions (id) ON DELETE CASCADE,
    merchant              TEXT,
    origin                TEXT    NOT NULL CHECK (origin IN ('user', 'correction')),
    origin_transaction_id INTEGER REFERENCES transactions (id) ON DELETE CASCADE,
    origin_decision_id    INTEGER REFERENCES decisions (id) ON DELETE CASCADE,
    priority              INTEGER NOT NULL,
    enabled               INTEGER NOT NULL CHECK (enabled IN (0, 1)),
    created_at            INTEGER NOT NULL,
    CHECK (CASE amount_kind
        WHEN 'exact'   THEN amount_min_minor IS NOT NULL AND amount_max_minor IS NULL AND amount_currency IS NOT NULL
        WHEN 'between' THEN amount_min_minor IS NOT NULL AND amount_max_minor IS NOT NULL AND amount_currency IS NOT NULL
        ELSE amount_kind IS NULL AND amount_min_minor IS NULL AND amount_max_minor IS NULL AND amount_currency IS NULL
    END),
    CHECK (CASE bucket
        WHEN 'category'     THEN bucket_category_id IS NOT NULL AND bucket_transaction_id IS NULL
        WHEN 'transfer'     THEN bucket_category_id IS NULL
        WHEN 'refund'       THEN bucket_category_id IS NOT NULL
        ELSE bucket_category_id IS NULL AND bucket_transaction_id IS NULL
    END),
    CHECK (bucket IS NOT NULL OR merchant IS NOT NULL),
    CHECK (CASE origin
        WHEN 'correction' THEN origin_transaction_id IS NOT NULL AND origin_decision_id IS NOT NULL
        ELSE origin_transaction_id IS NULL AND origin_decision_id IS NULL
    END)
) STRICT;
CREATE INDEX rules_by_origin_transaction ON rules (origin_transaction_id) WHERE origin_transaction_id IS NOT NULL;
CREATE INDEX rules_by_origin_decision ON rules (origin_decision_id) WHERE origin_decision_id IS NOT NULL;
CREATE INDEX rules_by_bucket_transaction ON rules (bucket_transaction_id) WHERE bucket_transaction_id IS NOT NULL;

-- Once a rule has made decisions, what it matches and does is fixed, so those decisions
-- keep describing it (constraint 8). Priority and enabled can still change; a different
-- rule is a new rule.
CREATE TRIGGER rules_are_frozen_once_used
BEFORE UPDATE ON rules
WHEN EXISTS (SELECT 1 FROM decisions WHERE rule_id = OLD.id)
 AND NOT (
        NEW.id IS OLD.id AND NEW.match_kind IS OLD.match_kind AND NEW.match_text IS OLD.match_text
    AND NEW.account_id IS OLD.account_id AND NEW.amount_kind IS OLD.amount_kind
    AND NEW.amount_min_minor IS OLD.amount_min_minor AND NEW.amount_max_minor IS OLD.amount_max_minor
    AND NEW.amount_currency IS OLD.amount_currency AND NEW.bucket IS OLD.bucket
    AND NEW.bucket_category_id IS OLD.bucket_category_id
    AND NEW.bucket_transaction_id IS OLD.bucket_transaction_id AND NEW.merchant IS OLD.merchant
    AND NEW.origin IS OLD.origin AND NEW.origin_transaction_id IS OLD.origin_transaction_id
    AND NEW.origin_decision_id IS OLD.origin_decision_id AND NEW.created_at IS OLD.created_at
 )
BEGIN
    SELECT RAISE(ABORT, 'immutable: a rule that has made decisions keeps its matcher and action');
END;

CREATE TABLE decisions (
    id                    INTEGER PRIMARY KEY,
    transaction_id        INTEGER NOT NULL REFERENCES transactions (id) ON DELETE CASCADE,
    assignment            TEXT    NOT NULL CHECK (assignment IN ('bucket', 'merchant')),
    bucket                TEXT    CHECK (bucket IN ('category', 'transfer', 'income', 'refund', 'needs_review')),
    bucket_category_id    INTEGER REFERENCES categories (id) ON DELETE RESTRICT,
    bucket_transaction_id INTEGER REFERENCES transactions (id) ON DELETE CASCADE,
    merchant              TEXT,
    source                TEXT    NOT NULL CHECK (source IN ('user', 'rule', 'builtin', 'model')),
    rule_id               INTEGER REFERENCES rules (id) ON DELETE CASCADE,
    builtin_logic         TEXT    CHECK (builtin_logic IN ('transfer_matcher', 'merchant_cleaner')),
    builtin_version       INTEGER CHECK (builtin_version BETWEEN 0 AND 4294967295),
    model_id              TEXT,
    model_gguf_sha256     BLOB    CHECK (length(model_gguf_sha256) = 32),
    prompt_version        TEXT,
    confidence_bp         INTEGER CHECK (confidence_bp BETWEEN 0 AND 10000),
    taxonomy_base         TEXT    NOT NULL,
    taxonomy_revision     INTEGER NOT NULL CHECK (taxonomy_revision >= 0),
    decided_at            INTEGER NOT NULL,
    state                 TEXT    NOT NULL CHECK (state IN ('applied', 'suggested', 'dismissed', 'superseded')),
    seen_at               INTEGER,
    replaces              INTEGER REFERENCES decisions (id) ON DELETE SET NULL,
    CHECK (CASE assignment
        WHEN 'merchant' THEN merchant IS NOT NULL AND bucket IS NULL
                             AND bucket_category_id IS NULL AND bucket_transaction_id IS NULL
        ELSE merchant IS NULL AND CASE bucket
            WHEN 'category'     THEN bucket_category_id IS NOT NULL AND bucket_transaction_id IS NULL
            WHEN 'transfer'     THEN bucket_category_id IS NULL
            WHEN 'refund'       THEN bucket_category_id IS NOT NULL
            WHEN 'income'       THEN bucket_category_id IS NULL AND bucket_transaction_id IS NULL
            WHEN 'needs_review' THEN bucket_category_id IS NULL AND bucket_transaction_id IS NULL
            ELSE 0
        END
    END),
    CHECK ((rule_id IS NOT NULL) = (source = 'rule')),
    CHECK ((builtin_logic IS NOT NULL AND builtin_version IS NOT NULL) = (source = 'builtin')),
    CHECK ((builtin_logic IS NULL) = (builtin_version IS NULL)),
    CHECK (CASE source
        WHEN 'model' THEN model_id IS NOT NULL AND model_gguf_sha256 IS NOT NULL
                          AND prompt_version IS NOT NULL AND confidence_bp IS NOT NULL
        ELSE model_id IS NULL AND model_gguf_sha256 IS NULL
             AND prompt_version IS NULL AND confidence_bp IS NULL
    END)
) STRICT;
CREATE INDEX decisions_by_transaction ON decisions (transaction_id, state);
CREATE INDEX decisions_by_rule ON decisions (rule_id) WHERE rule_id IS NOT NULL;
CREATE INDEX decisions_by_bucket_transaction ON decisions (bucket_transaction_id) WHERE bucket_transaction_id IS NOT NULL;
CREATE INDEX decisions_by_replaces ON decisions (replaces) WHERE replaces IS NOT NULL;

-- Decisions are append-only (T0.5): a change is a new decision. Outside a deletion only
-- `state` moves (applied → superseded; suggested → applied, dismissed or superseded) and
-- `seen_at` is set once, from NULL. An update that changes nothing is refused too.
-- During a deletion, the only change allowed is the cascade clearing `replaces`
-- (ON DELETE SET NULL).
CREATE TRIGGER decisions_are_append_only
BEFORE UPDATE ON decisions
WHEN NOT (
        NEW.id IS OLD.id AND NEW.transaction_id IS OLD.transaction_id
    AND NEW.assignment IS OLD.assignment AND NEW.bucket IS OLD.bucket
    AND NEW.bucket_category_id IS OLD.bucket_category_id
    AND NEW.bucket_transaction_id IS OLD.bucket_transaction_id
    AND NEW.merchant IS OLD.merchant AND NEW.source IS OLD.source
    AND NEW.rule_id IS OLD.rule_id AND NEW.builtin_logic IS OLD.builtin_logic
    AND NEW.builtin_version IS OLD.builtin_version AND NEW.model_id IS OLD.model_id
    AND NEW.model_gguf_sha256 IS OLD.model_gguf_sha256
    AND NEW.prompt_version IS OLD.prompt_version AND NEW.confidence_bp IS OLD.confidence_bp
    AND NEW.taxonomy_base IS OLD.taxonomy_base AND NEW.taxonomy_revision IS OLD.taxonomy_revision
    AND NEW.decided_at IS OLD.decided_at
    AND CASE WHEN fm_permit('delete')
        THEN NEW.replaces IS NULL AND OLD.replaces IS NOT NULL
         AND NEW.state IS OLD.state AND NEW.seen_at IS OLD.seen_at
        ELSE NEW.replaces IS OLD.replaces
         AND CASE
             -- A state transition, with seen_at unchanged or set for the first time.
             WHEN (OLD.state = 'applied'   AND NEW.state = 'superseded')
               OR (OLD.state = 'suggested' AND NEW.state IN ('applied', 'dismissed', 'superseded'))
             THEN NEW.seen_at IS OLD.seen_at OR (OLD.seen_at IS NULL AND NEW.seen_at IS NOT NULL)
             -- Or only seen_at, set for the first time.
             WHEN NEW.state IS OLD.state
             THEN OLD.seen_at IS NULL AND NEW.seen_at IS NOT NULL
             ELSE 0
         END
    END
)
BEGIN
    SELECT RAISE(ABORT, 'decisions are append-only: only state transitions and seen_at may change');
END;

CREATE TRIGGER decisions_are_never_deleted
BEFORE DELETE ON decisions
WHEN NOT fm_permit('delete')
BEGIN
    SELECT RAISE(ABORT, 'decisions are append-only: they are removed only by deleting their source data');
END;

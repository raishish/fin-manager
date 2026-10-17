-- Migration 3: source facts on transactions, and the import pipeline as a decision source.
--
-- 1. transactions gain trn_type (the source's own transaction type, e.g. OFX TRNTYPE)
--    and check_number: immutable facts that transfer detection (T1.4) and labelling
--    (paper checks) need.
-- 2. decisions.builtin_logic also accepts 'import_pipeline', the source of the Needs
--    review decision every new transaction gets (T1.3). SQLite can't change a CHECK in
--    place, so the table is rebuilt (https://sqlite.org/lang_altertable.html, "Making
--    Other Kinds Of Table Schema Changes"): the runner turns foreign keys off around
--    this migration, so dropping the old table can't cascade into rules, and runs
--    foreign_key_check before committing. Triggers and indexes on decisions are
--    recreated as in migration 2; the rules trigger that reads decisions is dropped and
--    recreated so the rename sees a consistent schema.

ALTER TABLE transactions ADD COLUMN trn_type TEXT;
ALTER TABLE transactions ADD COLUMN check_number TEXT;

DROP TRIGGER rules_are_frozen_once_used;

CREATE TABLE decisions_v3 (
    id                    INTEGER PRIMARY KEY,
    transaction_id        INTEGER NOT NULL REFERENCES transactions (id) ON DELETE CASCADE,
    assignment            TEXT    NOT NULL CHECK (assignment IN ('bucket', 'merchant')),
    bucket                TEXT    CHECK (bucket IN ('category', 'transfer', 'income', 'refund', 'needs_review')),
    bucket_category_id    INTEGER REFERENCES categories (id) ON DELETE RESTRICT,
    bucket_transaction_id INTEGER REFERENCES transactions (id) ON DELETE CASCADE,
    merchant              TEXT,
    source                TEXT    NOT NULL CHECK (source IN ('user', 'rule', 'builtin', 'model')),
    rule_id               INTEGER REFERENCES rules (id) ON DELETE CASCADE,
    builtin_logic         TEXT    CHECK (builtin_logic IN ('transfer_matcher', 'merchant_cleaner', 'import_pipeline')),
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

INSERT INTO decisions_v3 SELECT * FROM decisions;
DROP TABLE decisions;
ALTER TABLE decisions_v3 RENAME TO decisions;

CREATE INDEX decisions_by_transaction ON decisions (transaction_id, state);
CREATE INDEX decisions_by_rule ON decisions (rule_id) WHERE rule_id IS NOT NULL;
CREATE INDEX decisions_by_bucket_transaction ON decisions (bucket_transaction_id) WHERE bucket_transaction_id IS NOT NULL;
CREATE INDEX decisions_by_replaces ON decisions (replaces) WHERE replaces IS NOT NULL;

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

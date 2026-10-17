-- Migration 5: a user's rejection of a decision (SPEC decision 46).
--
-- A decision the user has acted on (seen_at set) may move applied → dismissed: the
-- user rejected it, which is different from it being replaced (superseded). An unseen
-- applied decision still can't be dismissed, and every other move stays as migration 3
-- left it. SQLite can't alter a trigger, so it is dropped and recreated whole.

DROP TRIGGER decisions_are_append_only;

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
             -- A user's rejection of a decision they have acted on (SPEC decision 46):
             -- applied → dismissed, only once seen, with seen_at kept.
             WHEN OLD.state = 'applied' AND NEW.state = 'dismissed'
             THEN OLD.seen_at IS NOT NULL AND NEW.seen_at IS OLD.seen_at
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

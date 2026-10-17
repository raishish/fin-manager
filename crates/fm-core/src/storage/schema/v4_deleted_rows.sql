-- Migration 4: rows deleted out of an original file that is kept (SPEC decision 43).
--
-- Deleting by account or date range removes the transactions in scope but may keep the
-- raw import they came from. Each such row is recorded here by its place in the file,
-- so nothing that re-reads the file brings it back. The records go with the file
-- (ON DELETE CASCADE). They are written only by the deletion API, never changed, and
-- removed only with their file.

CREATE TABLE deleted_rows (
    import_id    INTEGER NOT NULL REFERENCES raw_imports (id) ON DELETE CASCADE,
    source_index INTEGER NOT NULL CHECK (source_index BETWEEN 0 AND 4294967295),
    PRIMARY KEY (import_id, source_index)
) STRICT;

CREATE TRIGGER deleted_rows_are_recorded_only_by_deletion
BEFORE INSERT ON deleted_rows
WHEN NOT fm_permit('delete')
BEGIN
    SELECT RAISE(ABORT, 'immutable: deleted rows are recorded only by the deletion API');
END;

CREATE TRIGGER deleted_rows_never_change
BEFORE UPDATE ON deleted_rows
BEGIN
    SELECT RAISE(ABORT, 'immutable: a deleted row record never changes');
END;

CREATE TRIGGER deleted_rows_go_only_with_their_import
BEFORE DELETE ON deleted_rows
WHEN NOT fm_permit('delete')
BEGIN
    SELECT RAISE(ABORT, 'immutable: deleted row records are removed only with their import');
END;

-- Migration 6: ids are never reused (T0.14).
--
-- New ids are one more than the largest ever used in a table, not the largest still
-- there: once the newest rows are deleted (a rule and the decisions it made, a whole
-- import), a new row must not take an old id, or a screen still holding that id could
-- act on a different row. id_marks keeps, per table, the highest id ever inserted:
-- seeded from the ledger as it is, raised by every insert, and never lowered, removed
-- or added to.

CREATE TABLE id_marks (
    table_name TEXT    PRIMARY KEY NOT NULL CHECK (table_name IN ('accounts', 'raw_imports', 'transactions', 'category_groups', 'categories', 'decisions', 'rules')),
    high       INTEGER NOT NULL CHECK (high >= 0)
) STRICT;

INSERT INTO id_marks (table_name, high) SELECT 'accounts', coalesce(max(id), 0) FROM accounts;
INSERT INTO id_marks (table_name, high) SELECT 'raw_imports', coalesce(max(id), 0) FROM raw_imports;
INSERT INTO id_marks (table_name, high) SELECT 'transactions', coalesce(max(id), 0) FROM transactions;
INSERT INTO id_marks (table_name, high) SELECT 'category_groups', coalesce(max(id), 0) FROM category_groups;
INSERT INTO id_marks (table_name, high) SELECT 'categories', coalesce(max(id), 0) FROM categories;
INSERT INTO id_marks (table_name, high) SELECT 'decisions', coalesce(max(id), 0) FROM decisions;
INSERT INTO id_marks (table_name, high) SELECT 'rules', coalesce(max(id), 0) FROM rules;

CREATE TRIGGER id_marks_only_grow
BEFORE UPDATE ON id_marks
WHEN NEW.table_name IS NOT OLD.table_name OR NEW.high < OLD.high
BEGIN
    SELECT RAISE(ABORT, 'immutable: an id mark only grows');
END;

CREATE TRIGGER id_marks_are_never_deleted
BEFORE DELETE ON id_marks
BEGIN
    SELECT RAISE(ABORT, 'immutable: id marks are never deleted');
END;

CREATE TRIGGER id_marks_are_fixed
BEFORE INSERT ON id_marks
BEGIN
    SELECT RAISE(ABORT, 'immutable: the set of id marks is fixed');
END;

CREATE TRIGGER accounts_raise_id_mark
AFTER INSERT ON accounts
BEGIN
    UPDATE id_marks SET high = NEW.id WHERE table_name = 'accounts' AND high < NEW.id;
END;

CREATE TRIGGER raw_imports_raise_id_mark
AFTER INSERT ON raw_imports
BEGIN
    UPDATE id_marks SET high = NEW.id WHERE table_name = 'raw_imports' AND high < NEW.id;
END;

CREATE TRIGGER transactions_raise_id_mark
AFTER INSERT ON transactions
BEGIN
    UPDATE id_marks SET high = NEW.id WHERE table_name = 'transactions' AND high < NEW.id;
END;

CREATE TRIGGER category_groups_raise_id_mark
AFTER INSERT ON category_groups
BEGIN
    UPDATE id_marks SET high = NEW.id WHERE table_name = 'category_groups' AND high < NEW.id;
END;

CREATE TRIGGER categories_raise_id_mark
AFTER INSERT ON categories
BEGIN
    UPDATE id_marks SET high = NEW.id WHERE table_name = 'categories' AND high < NEW.id;
END;

CREATE TRIGGER decisions_raise_id_mark
AFTER INSERT ON decisions
BEGIN
    UPDATE id_marks SET high = NEW.id WHERE table_name = 'decisions' AND high < NEW.id;
END;

CREATE TRIGGER rules_raise_id_mark
AFTER INSERT ON rules
BEGIN
    UPDATE id_marks SET high = NEW.id WHERE table_name = 'rules' AND high < NEW.id;
END;

-- Run after 0001_init.sql in a fresh PostgreSQL database. All fixtures roll back.
BEGIN;

DO $verify$
DECLARE
    extracted TEXT;
    blocked BOOLEAN;
BEGIN
    IF (SELECT version FROM control_schema_version) <> 1 THEN
        RAISE EXCEPTION 'unexpected control schema version';
    END IF;

    IF json_valid('not json') OR NOT json_valid('{"ok":true}') THEN
        RAISE EXCEPTION 'JSON validation differs from SQLite';
    END IF;

    INSERT INTO decisions (at_ms, actor, op, evidence)
    VALUES (1, 'schema-test', 'sync', '{"sync":"OTK:1"}')
    RETURNING sync_id INTO extracted;
    IF extracted IS DISTINCT FROM 'OTK:1' THEN
        RAISE EXCEPTION 'decision sync ID was not generated';
    END IF;

    blocked := FALSE;
    BEGIN
        INSERT INTO decisions (at_ms, actor, op, evidence)
        VALUES (1, 'schema-test', 'bad-json', 'not json');
    -- The generated sync_id may parse the JSON before the CHECK runs.
    EXCEPTION WHEN check_violation OR invalid_text_representation THEN
        blocked := TRUE;
    END;
    IF NOT blocked THEN
        RAISE EXCEPTION 'invalid decision JSON was accepted';
    END IF;

    INSERT INTO users (id, email, role, created_at_ms, updated_at_ms)
    VALUES ('schema-test-1', 'Admin@Example.com', 'admin', 1, 1);
    blocked := FALSE;
    BEGIN
        INSERT INTO users (id, email, role, created_at_ms, updated_at_ms)
        VALUES ('schema-test-2', 'admin@example.com', 'admin', 1, 1);
    EXCEPTION WHEN unique_violation THEN
        blocked := TRUE;
    END;
    IF NOT blocked THEN
        RAISE EXCEPTION 'case-insensitive email uniqueness was not enforced';
    END IF;

    INSERT INTO audit (seq, at_ms, actor, op, outcome, detail, prev_hash, hash)
    VALUES (1, 1, 'schema-test', 'verify', 'success', '{}', '', 'hash');

    blocked := FALSE;
    BEGIN
        UPDATE audit SET detail = '{"changed":true}' WHERE seq = 1;
    EXCEPTION WHEN raise_exception THEN
        blocked := TRUE;
    END;
    IF NOT blocked THEN
        RAISE EXCEPTION 'audit UPDATE was accepted';
    END IF;

    blocked := FALSE;
    BEGIN
        DELETE FROM audit WHERE seq = 1;
    EXCEPTION WHEN raise_exception THEN
        blocked := TRUE;
    END;
    IF NOT blocked THEN
        RAISE EXCEPTION 'audit DELETE was accepted without the purge gate';
    END IF;

    blocked := FALSE;
    BEGIN
        TRUNCATE audit;
    EXCEPTION WHEN raise_exception THEN
        blocked := TRUE;
    END;
    IF NOT blocked THEN
        RAISE EXCEPTION 'audit TRUNCATE was accepted';
    END IF;

    INSERT INTO audit_purge_gate (id) VALUES (1);
    DELETE FROM audit WHERE seq = 1;
    IF EXISTS (SELECT 1 FROM audit WHERE seq = 1) THEN
        RAISE EXCEPTION 'authorized audit purge did not delete the row';
    END IF;
END
$verify$;

ROLLBACK;

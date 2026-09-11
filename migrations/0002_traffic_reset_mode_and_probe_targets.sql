-- Schema 2: storage foundation only. No behaviour in this migration.
--
-- Strictly additive: three ALTER TABLE ADD COLUMN statements, no rebuild, no
-- RENAME, no DROP, no foreign_keys or legacy_alter_table pragma. SQLite carries
-- the declared defaults into every existing row, so a v0.1.2 database keeps
-- exactly the behaviour it had: every node monthly, every target ICMP with no
-- port.

ALTER TABLE nodes ADD COLUMN traffic_reset_mode TEXT NOT NULL DEFAULT 'monthly'
    CHECK (traffic_reset_mode IN ('monthly', 'never'));

-- probe_kind is added before port on purpose: the port constraint below
-- references it, and a CHECK cannot name a column that does not exist yet.
ALTER TABLE ping_targets ADD COLUMN probe_kind TEXT NOT NULL DEFAULT 'icmp'
    CHECK (probe_kind IN ('icmp', 'tcp'));

-- NULL is not comparable, so requiring "port IS NOT NULL" for TCP is written
-- out rather than left to BETWEEN: a bare range test passes for NULL under
-- three-valued logic and would let a TCP target exist without a port.
ALTER TABLE ping_targets ADD COLUMN port INTEGER
    CHECK (
        (probe_kind = 'tcp' AND port IS NOT NULL AND port BETWEEN 1 AND 65535)
        OR
        (probe_kind = 'icmp' AND port IS NULL)
    );

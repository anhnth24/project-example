-- Phase: 1B
-- Owner: retrieval-owner, security-owner
-- Change: expand
-- Lock/data risk: additive CHECK + DEFAULT change on empty/low-traffic table.
-- Rollback compatibility: drop new CHECK; restore DEFAULT now().
--
-- Download capability issuance/consume must use PostgreSQL clock_timestamp()
-- (see repository SQL). Harden table defaults and max TTL against app clocks.

ALTER TABLE download_capabilities
    ALTER COLUMN created_at SET DEFAULT clock_timestamp();

ALTER TABLE download_capabilities
    ADD CONSTRAINT ck_download_capabilities__max_ttl
        CHECK (expires_at <= created_at + interval '300 seconds');

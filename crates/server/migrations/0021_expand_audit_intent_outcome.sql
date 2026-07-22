-- Phase: 1B
-- Owner: ops-owner
-- Change: expand
-- Lock/data risk: single CHECK swap on audit_log; widens the allowed set only
--                 (existing rows always satisfy a superset), brief ACCESS EXCLUSIVE.
-- Rollback compatibility: restore the three-value CHECK once no 'intent' rows remain.
--
-- The destructive object-cleanup audit trail (services/deletion.rs and
-- services/reconciliation.rs) records an 'intent' row before deleting objects,
-- but audit_log.outcome (migration 0009) only admitted 'success'/'deny'/'error',
-- so those audit writes failed at runtime with audit_log_outcome_check. Widen the
-- constraint to admit the intent marker; the value set is otherwise unchanged.

ALTER TABLE audit_log DROP CONSTRAINT audit_log_outcome_check;
ALTER TABLE audit_log
    ADD CONSTRAINT audit_log_outcome_check
    CHECK (outcome IN ('success', 'deny', 'error', 'intent'));

//! Canonical collection ACL predicate for authoritative Q&A / retrieval probes.
//!
//! A principal may read a collection when any of:
//! - `visibility = 'org'`
//! - owner
//! - direct `collection_user_access`
//! - group grant via `collection_group_access` + `group_memberships`
//! - role grant via `collection_role_access` + org membership role

/// SQL boolean for collection alias `acl_c` readable by bind `user_bind` (e.g. `$4`).
pub fn collection_readable_predicate(user_bind: &str) -> String {
    format!(
        "(
                     acl_c.visibility = 'org'
                     OR acl_c.owner_user_id = {user_bind}
                     OR EXISTS (
                       SELECT 1 FROM collection_user_access cua
                       WHERE cua.org_id = acl_c.org_id
                         AND cua.collection_id = acl_c.id
                         AND cua.user_id = {user_bind}
                     )
                     OR EXISTS (
                       SELECT 1 FROM collection_group_access cga
                       JOIN group_memberships gm
                         ON gm.org_id = cga.org_id
                        AND gm.group_id = cga.group_id
                        AND gm.user_id = {user_bind}
                       WHERE cga.org_id = acl_c.org_id
                         AND cga.collection_id = acl_c.id
                     )
                     OR EXISTS (
                       SELECT 1 FROM collection_role_access cra
                       JOIN org_memberships om
                         ON om.org_id = cra.org_id
                        AND om.user_id = {user_bind}
                       JOIN roles rr
                         ON rr.org_id = om.org_id
                        AND rr.code = om.role
                        AND rr.id = cra.role_id
                       WHERE cra.org_id = acl_c.org_id
                         AND cra.collection_id = acl_c.id
                     )
                   )"
    )
}

/// Same predicate for collections aliased as `c`.
pub fn collection_readable_predicate_c(user_bind: &str) -> String {
    collection_readable_predicate(user_bind).replace("acl_c", "c")
}

/// Canonical `AND <predicate>` fragment for alias `acl_c`.
pub fn and_collection_readable(user_bind: &str) -> String {
    format!("AND {}", collection_readable_predicate(user_bind))
}

/// Replace a legacy incomplete ACL block (org/owner/user only) marker with the
/// canonical predicate. Prefer building SQL via [`and_collection_readable`].
pub fn patch_legacy_acl_block(sql: &str, user_bind: &str) -> String {
    // Idempotent: if group/role already present, leave as-is.
    if sql.contains("collection_group_access") && sql.contains("collection_role_access") {
        return sql.to_string();
    }
    let legacy = format!(
        "(
                     acl_c.visibility = 'org'
                     OR acl_c.owner_user_id = {user_bind}
                     OR EXISTS (
                       SELECT 1 FROM collection_user_access cua
                       WHERE cua.org_id = acl_c.org_id
                         AND cua.collection_id = acl_c.id
                         AND cua.user_id = {user_bind}
                     )
                   )"
    );
    sql.replace(&legacy, &collection_readable_predicate(user_bind))
}

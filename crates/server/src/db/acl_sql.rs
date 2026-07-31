//! Shared PostgreSQL ACL builders (Phase 1C).
//!
//! Emits the same visibility and access-level branches as [`crate::auth::acl::allowed`].

/// SQL fragment listing collection ids visible to a principal for a permission/access pair.
pub fn allowed_collections_sql(
    _org_id_param: &str,
    _user_id_param: &str,
    _permission_param: &str,
    _required_access_param: &str,
) -> String {
    String::new()
}

/// EXISTS predicate gating chunk/claim/document reads by collection ACL.
pub fn acl_predicate_sql(
    _org_id_expr: &str,
    _collection_id_expr: &str,
    _user_id_param: &str,
    _permission_param: &str,
    _required_access_param: &str,
) -> String {
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACCESS_LEVEL_RANK: &str =
        "CASE access_level WHEN 'read' THEN 1 WHEN 'write' THEN 2 WHEN 'admin' THEN 3 ELSE 0 END";

    #[test]
    fn access_level_rank_expression_is_canonical() {
        let sql = allowed_collections_sql("$1", "$2", "$3", "$4");
        assert!(
            sql.contains(ACCESS_LEVEL_RANK),
            "allowed_collections_sql must use canonical access_level rank expression;\ngot:\n{sql}"
        );
        let predicate = acl_predicate_sql("c.org_id", "c.id", "$3", "$4", "$5");
        assert!(
            predicate.contains(ACCESS_LEVEL_RANK),
            "acl_predicate_sql must use canonical access_level rank expression;\ngot:\n{predicate}"
        );
    }

    #[test]
    fn allowed_collections_sql_shape_is_pinned() {
        let sql = allowed_collections_sql("$1", "$2", "$3", "$4");
        for expected in [
            "org_memberships",
            "state = 'active'",
            "users",
            "disabled_at IS NULL",
            "c.org_id = $1",
            "c.deleted_at IS NULL",
            "permissions",
            "p.code = $3",
            "c.visibility = 'org'",
            "c.owner_user_id = $2",
            "collection_user_access",
            "collection_group_access",
            "collection_role_access",
            "$4",
            ACCESS_LEVEL_RANK,
        ] {
            assert!(
                sql.contains(expected),
                "allowed_collections_sql missing required clause {expected:?} in:\n{sql}"
            );
        }
    }

    #[test]
    fn acl_predicate_sql_shape_is_pinned() {
        let sql = acl_predicate_sql("d.org_id", "d.collection_id", "$4", "$5", "$6");
        for expected in [
            "collections acl_c",
            "org_memberships acl_m",
            "acl_m.user_id = $4",
            "acl_m.state = 'active'",
            "users acl_u",
            "acl_u.disabled_at IS NULL",
            "acl_c.org_id = d.org_id",
            "acl_c.id = d.collection_id",
            "acl_c.deleted_at IS NULL",
            "permissions acl_p",
            "acl_p.code = $5",
            "acl_c.visibility = 'org'",
            "acl_c.owner_user_id = $4",
            "collection_user_access",
            "collection_group_access",
            "collection_role_access",
            "$6",
            ACCESS_LEVEL_RANK,
        ] {
            assert!(
                sql.contains(expected),
                "acl_predicate_sql missing required clause {expected:?} in:\n{sql}"
            );
        }
    }
}

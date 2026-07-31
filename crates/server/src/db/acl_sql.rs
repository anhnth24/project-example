//! Shared PostgreSQL ACL builders (Phase 1C).
//!
//! Emits the same visibility and access-level branches as [`crate::auth::acl::allowed`].

const ACCESS_LEVEL_RANK: &str =
    "CASE access_level WHEN 'read' THEN 1 WHEN 'write' THEN 2 WHEN 'admin' THEN 3 ELSE 0 END";

fn required_access_rank(required_access_param: &str) -> String {
    format!(
        "CASE {required_access_param} WHEN 'read' THEN 1 WHEN 'write' THEN 2 WHEN 'admin' THEN 3 ELSE 0 END"
    )
}

fn grant_meets_required(required_access_param: &str) -> String {
    format!(
        "({ACCESS_LEVEL_RANK}) >= ({})",
        required_access_rank(required_access_param)
    )
}

fn visibility_route_sql(user_id_param: &str, required_access_param: &str) -> String {
    let grant_meets = grant_meets_required(required_access_param);
    format!(
        "(
           c.visibility = 'org'
           OR c.owner_user_id = {user_id_param}
           OR (
             c.visibility = 'private'
             AND EXISTS (
               SELECT 1 FROM collection_user_access cua
               WHERE cua.org_id = c.org_id
                 AND cua.collection_id = c.id
                 AND cua.user_id = {user_id_param}
                 AND {grant_meets}
             )
           )
           OR (
             c.visibility = 'groups'
             AND (
               EXISTS (
                 SELECT 1 FROM collection_user_access cua
                 WHERE cua.org_id = c.org_id
                   AND cua.collection_id = c.id
                   AND cua.user_id = {user_id_param}
                   AND {grant_meets}
               )
               OR EXISTS (
                 SELECT 1 FROM collection_group_access cga
                 JOIN group_memberships gm
                   ON gm.org_id = cga.org_id
                  AND gm.group_id = cga.group_id
                  AND gm.user_id = {user_id_param}
                 WHERE cga.org_id = c.org_id
                   AND cga.collection_id = c.id
                   AND {grant_meets}
               )
               OR EXISTS (
                 SELECT 1 FROM collection_role_access cra
                 JOIN roles cra_r
                   ON cra_r.org_id = cra.org_id
                  AND cra_r.id = cra.role_id
                 JOIN org_memberships cra_m
                   ON cra_m.org_id = cra_r.org_id
                  AND cra_m.user_id = {user_id_param}
                  AND cra_m.role = cra_r.code
                  AND cra_m.state = 'active'
                 WHERE cra.org_id = c.org_id
                   AND cra.collection_id = c.id
                   AND {grant_meets}
               )
             )
           )
         )"
    )
}

fn membership_permission_exists_sql(
    org_id_param: &str,
    user_id_param: &str,
    permission_param: &str,
) -> String {
    format!(
        "EXISTS (
           SELECT 1
           FROM org_memberships m
           JOIN users u ON u.id = m.user_id
           JOIN roles r
             ON r.org_id = m.org_id AND r.code = m.role
           JOIN role_permissions rp
             ON rp.org_id = r.org_id AND rp.role_id = r.id
           JOIN permissions p ON p.id = rp.permission_id
           WHERE m.org_id = {org_id_param}
             AND m.user_id = {user_id_param}
             AND m.state = 'active'
             AND u.disabled_at IS NULL
             AND p.code = {permission_param}
         )"
    )
}

/// SQL fragment listing collection ids visible to a principal for a permission/access pair.
pub fn allowed_collections_sql(
    org_id_param: &str,
    user_id_param: &str,
    permission_param: &str,
    required_access_param: &str,
) -> String {
    let membership =
        membership_permission_exists_sql(org_id_param, user_id_param, permission_param);
    let visibility = visibility_route_sql(user_id_param, required_access_param);
    format!(
        "c.org_id = {org_id_param}
         AND c.deleted_at IS NULL
         AND {membership}
         AND {visibility}"
    )
}

/// EXISTS predicate gating chunk/claim/document reads by collection ACL.
pub fn acl_predicate_sql(
    org_id_expr: &str,
    collection_id_expr: &str,
    user_id_param: &str,
    permission_param: &str,
    required_access_param: &str,
) -> String {
    let grant_meets = grant_meets_required(required_access_param);
    format!(
        "EXISTS (
           SELECT 1
           FROM collections acl_c
           JOIN org_memberships acl_m
             ON acl_m.org_id = acl_c.org_id AND acl_m.user_id = {user_id_param}
            AND acl_m.state = 'active'
           JOIN users acl_u ON acl_u.id = acl_m.user_id
           JOIN roles acl_r
             ON acl_r.org_id = acl_m.org_id AND acl_r.code = acl_m.role
           JOIN role_permissions acl_rp
             ON acl_rp.org_id = acl_r.org_id AND acl_rp.role_id = acl_r.id
           JOIN permissions acl_p ON acl_p.id = acl_rp.permission_id
           WHERE acl_c.org_id = {org_id_expr}
             AND acl_c.id = {collection_id_expr}
             AND acl_c.deleted_at IS NULL
             AND acl_u.disabled_at IS NULL
             AND acl_p.code = {permission_param}
             AND (
               acl_c.visibility = 'org'
               OR acl_c.owner_user_id = {user_id_param}
               OR (
                 acl_c.visibility = 'private'
                 AND EXISTS (
                   SELECT 1 FROM collection_user_access cua
                   WHERE cua.org_id = acl_c.org_id
                     AND cua.collection_id = acl_c.id
                     AND cua.user_id = {user_id_param}
                     AND {grant_meets}
                 )
               )
               OR (
                 acl_c.visibility = 'groups'
                 AND (
                   EXISTS (
                     SELECT 1 FROM collection_user_access cua
                     WHERE cua.org_id = acl_c.org_id
                       AND cua.collection_id = acl_c.id
                       AND cua.user_id = {user_id_param}
                       AND {grant_meets}
                   )
                   OR EXISTS (
                     SELECT 1 FROM collection_group_access cga
                     JOIN group_memberships gm
                       ON gm.org_id = cga.org_id
                      AND gm.group_id = cga.group_id
                      AND gm.user_id = {user_id_param}
                     WHERE cga.org_id = acl_c.org_id
                       AND cga.collection_id = acl_c.id
                       AND {grant_meets}
                   )
                   OR EXISTS (
                     SELECT 1 FROM collection_role_access cra
                     JOIN roles cra_r
                       ON cra_r.org_id = cra.org_id
                      AND cra_r.id = cra.role_id
                     JOIN org_memberships cra_m
                       ON cra_m.org_id = cra_r.org_id
                      AND cra_m.user_id = {user_id_param}
                      AND cra_m.role = cra_r.code
                      AND cra_m.state = 'active'
                     WHERE cra.org_id = acl_c.org_id
                       AND cra.collection_id = acl_c.id
                       AND {grant_meets}
                   )
                 )
               )
             )
         )"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACCESS_LEVEL_RANK: &str =
        "CASE access_level WHEN 'read' THEN 1 WHEN 'write' THEN 2 WHEN 'admin' THEN 3 ELSE 0 END";

    fn visibility_arm<'a>(sql: &'a str, collection_alias: &str, visibility: &str) -> &'a str {
        let marker = format!("{collection_alias}.visibility = '{visibility}'");
        let start = sql
            .find(&marker)
            .unwrap_or_else(|| panic!("missing visibility marker {marker:?} in:\n{sql}"));
        let end = if visibility == "private" {
            let groups_marker = format!("{collection_alias}.visibility = 'groups'");
            sql[start..]
                .find(&groups_marker)
                .map(|offset| start + offset)
                .unwrap_or(sql.len())
        } else {
            sql.len()
        };
        &sql[start..end]
    }

    fn assert_private_arm_isolated(sql: &str, collection_alias: &str) {
        let arm = visibility_arm(sql, collection_alias, "private");
        assert!(
            arm.contains("collection_user_access"),
            "private arm must allow direct user grants:\n{arm}"
        );
        for forbidden in [
            "collection_group_access",
            "collection_role_access",
            "group_memberships",
        ] {
            assert!(
                !arm.contains(forbidden),
                "private arm must not reference {forbidden}:\n{arm}"
            );
        }
    }

    fn assert_groups_arm_has_grant_branches(sql: &str, collection_alias: &str) {
        let arm = visibility_arm(sql, collection_alias, "groups");
        for required in [
            "collection_user_access",
            "collection_group_access",
            "collection_role_access",
            "group_memberships",
        ] {
            assert!(
                arm.contains(required),
                "groups arm must reference {required}:\n{arm}"
            );
        }
    }

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

    #[test]
    fn visibility_markers_are_pinned_in_allowed_collections_sql() {
        let sql = allowed_collections_sql("$1", "$2", "$3", "$4");
        for marker in ["c.visibility = 'org'", "c.visibility = 'private'", "c.visibility = 'groups'"]
        {
            assert!(
                sql.contains(marker),
                "allowed_collections_sql missing visibility marker {marker:?} in:\n{sql}"
            );
        }
    }

    #[test]
    fn visibility_markers_are_pinned_in_acl_predicate_sql() {
        let sql = acl_predicate_sql("d.org_id", "d.collection_id", "$4", "$5", "$6");
        for marker in [
            "acl_c.visibility = 'org'",
            "acl_c.visibility = 'private'",
            "acl_c.visibility = 'groups'",
        ] {
            assert!(
                sql.contains(marker),
                "acl_predicate_sql missing visibility marker {marker:?} in:\n{sql}"
            );
        }
    }

    #[test]
    fn allowed_collections_private_arm_has_no_group_or_role_leak() {
        let sql = allowed_collections_sql("$1", "$2", "$3", "$4");
        assert_private_arm_isolated(&sql, "c");
    }

    #[test]
    fn acl_predicate_private_arm_has_no_group_or_role_leak() {
        let sql = acl_predicate_sql("d.org_id", "d.collection_id", "$4", "$5", "$6");
        assert_private_arm_isolated(&sql, "acl_c");
    }

    #[test]
    fn allowed_collections_groups_arm_has_group_and_role_branches() {
        let sql = allowed_collections_sql("$1", "$2", "$3", "$4");
        assert_groups_arm_has_grant_branches(&sql, "c");
    }

    #[test]
    fn acl_predicate_groups_arm_has_group_and_role_branches() {
        let sql = acl_predicate_sql("d.org_id", "d.collection_id", "$4", "$5", "$6");
        assert_groups_arm_has_grant_branches(&sql, "acl_c");
    }
}

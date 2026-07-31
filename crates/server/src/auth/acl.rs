//! Canonical collection ACL semantics (Phase 1C).
//!
//! Pure predicate reference consumed by the org-context resolver, upload saga,
//! FTS/hydration, and direct operation guards. PostgreSQL builders live in
//! [`crate::db::acl_sql`].

use std::collections::BTreeSet;

use uuid::Uuid;

use crate::db::models::{AccessLevel, CollectionVisibility};

/// Principal snapshot for ACL evaluation (membership, user state, base permissions).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AclPrincipal {
    pub org_id: Uuid,
    pub user_id: Uuid,
    pub membership_active: bool,
    pub user_disabled: bool,
    pub permissions: BTreeSet<String>,
}

/// Collection ACL snapshot for pure predicate evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionAclSnapshot {
    pub collection_id: Uuid,
    pub org_id: Uuid,
    pub owner_user_id: Uuid,
    pub visibility: CollectionVisibility,
    pub user_grant: Option<AccessLevel>,
    pub group_grants: Vec<AccessLevel>,
    pub role_grant: Option<AccessLevel>,
}

/// Fail-closed collection ACL predicate (semantic reference).
pub fn allowed(
    principal: &AclPrincipal,
    collection: &CollectionAclSnapshot,
    permission: &str,
    required_access: AccessLevel,
) -> bool {
    if !principal.membership_active
        || principal.user_disabled
        || principal.org_id != collection.org_id
        || !principal.permissions.contains(permission)
    {
        return false;
    }
    let grant_allows = |grant: Option<AccessLevel>| {
        grant.is_some_and(|level| level.satisfies(required_access))
    };
    match collection.visibility {
        CollectionVisibility::Private => {
            principal.user_id == collection.owner_user_id
                || grant_allows(collection.user_grant)
        }
        CollectionVisibility::Org => true,
        CollectionVisibility::Groups => {
            principal.user_id == collection.owner_user_id
                || grant_allows(collection.user_grant)
                || collection
                    .group_grants
                    .iter()
                    .copied()
                    .any(|level| level.satisfies(required_access))
                || grant_allows(collection.role_grant)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PERMISSION: &str = "qa.query";

    fn org_id() -> Uuid {
        Uuid::from_u128(1)
    }

    fn user_id() -> Uuid {
        Uuid::from_u128(2)
    }

    fn other_user_id() -> Uuid {
        Uuid::from_u128(3)
    }

    fn collection_id() -> Uuid {
        Uuid::from_u128(4)
    }

    fn principal_with_permissions(permissions: &[&str]) -> AclPrincipal {
        AclPrincipal {
            org_id: org_id(),
            user_id: user_id(),
            membership_active: true,
            user_disabled: false,
            permissions: permissions.iter().map(|p| (*p).to_string()).collect(),
        }
    }

    fn collection_snapshot(visibility: CollectionVisibility) -> CollectionAclSnapshot {
        CollectionAclSnapshot {
            collection_id: collection_id(),
            org_id: org_id(),
            owner_user_id: other_user_id(),
            visibility,
            user_grant: None,
            group_grants: Vec::new(),
            role_grant: None,
        }
    }

    #[test]
    fn inactive_membership_denies() {
        let mut principal = principal_with_permissions(&[PERMISSION]);
        principal.membership_active = false;
        let collection = collection_snapshot(CollectionVisibility::Org);
        assert!(!allowed(
            &principal,
            &collection,
            PERMISSION,
            AccessLevel::Read
        ));
    }

    #[test]
    fn disabled_user_denies() {
        let mut principal = principal_with_permissions(&[PERMISSION]);
        principal.user_disabled = true;
        let collection = collection_snapshot(CollectionVisibility::Org);
        assert!(!allowed(
            &principal,
            &collection,
            PERMISSION,
            AccessLevel::Read
        ));
    }

    #[test]
    fn missing_base_permission_denies() {
        let principal = principal_with_permissions(&[]);
        let collection = collection_snapshot(CollectionVisibility::Org);
        assert!(!allowed(
            &principal,
            &collection,
            PERMISSION,
            AccessLevel::Read
        ));
    }

    #[test]
    fn org_mismatch_denies() {
        let principal = principal_with_permissions(&[PERMISSION]);
        let mut collection = collection_snapshot(CollectionVisibility::Org);
        collection.org_id = Uuid::from_u128(99);
        assert!(!allowed(
            &principal,
            &collection,
            PERMISSION,
            AccessLevel::Read
        ));
    }

    #[test]
    fn private_ignores_group_grant() {
        let principal = principal_with_permissions(&[PERMISSION]);
        let mut collection = collection_snapshot(CollectionVisibility::Private);
        collection.group_grants = vec![AccessLevel::Admin];
        assert!(!allowed(
            &principal,
            &collection,
            PERMISSION,
            AccessLevel::Read
        ));
    }

    #[test]
    fn private_ignores_role_grant() {
        let principal = principal_with_permissions(&[PERMISSION]);
        let mut collection = collection_snapshot(CollectionVisibility::Private);
        collection.role_grant = Some(AccessLevel::Admin);
        assert!(!allowed(
            &principal,
            &collection,
            PERMISSION,
            AccessLevel::Read
        ));
    }

    #[test]
    fn private_allows_owner() {
        let principal = principal_with_permissions(&[PERMISSION]);
        let mut collection = collection_snapshot(CollectionVisibility::Private);
        collection.owner_user_id = principal.user_id;
        assert!(allowed(
            &principal,
            &collection,
            PERMISSION,
            AccessLevel::Read
        ));
    }

    #[test]
    fn private_allows_sufficient_direct_user_grant() {
        let principal = principal_with_permissions(&[PERMISSION]);
        let mut collection = collection_snapshot(CollectionVisibility::Private);
        collection.user_grant = Some(AccessLevel::Write);
        assert!(allowed(
            &principal,
            &collection,
            PERMISSION,
            AccessLevel::Read
        ));
    }

    #[test]
    fn private_denies_insufficient_direct_user_grant() {
        let principal = principal_with_permissions(&[PERMISSION]);
        let mut collection = collection_snapshot(CollectionVisibility::Private);
        collection.user_grant = Some(AccessLevel::Read);
        assert!(!allowed(
            &principal,
            &collection,
            PERMISSION,
            AccessLevel::Write
        ));
    }

    #[test]
    fn org_allows_active_member_with_base_permission() {
        let principal = principal_with_permissions(&[PERMISSION]);
        let collection = collection_snapshot(CollectionVisibility::Org);
        assert!(allowed(
            &principal,
            &collection,
            PERMISSION,
            AccessLevel::Read
        ));
    }

    #[test]
    fn groups_allows_sufficient_user_grant() {
        let principal = principal_with_permissions(&[PERMISSION]);
        let mut collection = collection_snapshot(CollectionVisibility::Groups);
        collection.user_grant = Some(AccessLevel::Write);
        assert!(allowed(
            &principal,
            &collection,
            PERMISSION,
            AccessLevel::Read
        ));
    }

    #[test]
    fn groups_allows_sufficient_group_grant() {
        let principal = principal_with_permissions(&[PERMISSION]);
        let mut collection = collection_snapshot(CollectionVisibility::Groups);
        collection.group_grants = vec![AccessLevel::Write];
        assert!(allowed(
            &principal,
            &collection,
            PERMISSION,
            AccessLevel::Read
        ));
    }

    #[test]
    fn groups_allows_sufficient_role_grant() {
        let principal = principal_with_permissions(&[PERMISSION]);
        let mut collection = collection_snapshot(CollectionVisibility::Groups);
        collection.role_grant = Some(AccessLevel::Write);
        assert!(allowed(
            &principal,
            &collection,
            PERMISSION,
            AccessLevel::Read
        ));
    }

    #[test]
    fn groups_allows_owner_without_explicit_grant() {
        let principal = principal_with_permissions(&[PERMISSION]);
        let mut collection = collection_snapshot(CollectionVisibility::Groups);
        collection.owner_user_id = principal.user_id;
        assert!(allowed(
            &principal,
            &collection,
            PERMISSION,
            AccessLevel::Read
        ));
    }

    #[test]
    fn access_level_ordering_read_write_admin() {
        assert!(AccessLevel::Read.satisfies(AccessLevel::Read));
        assert!(AccessLevel::Write.satisfies(AccessLevel::Read));
        assert!(AccessLevel::Write.satisfies(AccessLevel::Write));
        assert!(AccessLevel::Admin.satisfies(AccessLevel::Read));
        assert!(AccessLevel::Admin.satisfies(AccessLevel::Write));
        assert!(AccessLevel::Admin.satisfies(AccessLevel::Admin));
        assert!(!AccessLevel::Read.satisfies(AccessLevel::Write));
        assert!(!AccessLevel::Read.satisfies(AccessLevel::Admin));
        assert!(!AccessLevel::Write.satisfies(AccessLevel::Admin));
    }

    #[test]
    fn read_grant_never_satisfies_write_or_admin() {
        let principal = principal_with_permissions(&[PERMISSION]);
        let mut collection = collection_snapshot(CollectionVisibility::Groups);
        collection.user_grant = Some(AccessLevel::Read);
        assert!(!allowed(
            &principal,
            &collection,
            PERMISSION,
            AccessLevel::Write
        ));
        assert!(!allowed(
            &principal,
            &collection,
            PERMISSION,
            AccessLevel::Admin
        ));
    }
}

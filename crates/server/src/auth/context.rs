//! Fail-closed tenant scope for every business repository call (ADR 0007).

use std::collections::BTreeSet;

use thiserror::Error;
use uuid::Uuid;

/// Explicit org/user scope required by every business repository method.
///
/// Construction is fail-closed: a missing or nil `org_id` / `user_id` never yields
/// a usable context. `allowed_collection_ids` is a plain field here; full ACL
/// resolution belongs to Phase 1C.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrgContext {
    org_id: Uuid,
    user_id: Uuid,
    permissions: BTreeSet<String>,
    allowed_collection_ids: BTreeSet<Uuid>,
}

/// Errors when constructing an [`OrgContext`].
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum OrgContextError {
    #[error("org_id is required and must not be nil")]
    MissingOrgId,
    #[error("user_id is required and must not be nil")]
    MissingUserId,
}

impl OrgContext {
    /// Builds a tenant context or rejects empty/nil scope.
    pub fn try_new(
        org_id: Uuid,
        user_id: Uuid,
        permissions: impl IntoIterator<Item = impl Into<String>>,
        allowed_collection_ids: impl IntoIterator<Item = Uuid>,
    ) -> Result<Self, OrgContextError> {
        if org_id.is_nil() {
            return Err(OrgContextError::MissingOrgId);
        }
        if user_id.is_nil() {
            return Err(OrgContextError::MissingUserId);
        }
        Ok(Self {
            org_id,
            user_id,
            permissions: permissions.into_iter().map(Into::into).collect(),
            allowed_collection_ids: allowed_collection_ids.into_iter().collect(),
        })
    }

    /// Tenant organization id (never nil after construction).
    pub fn org_id(&self) -> Uuid {
        self.org_id
    }

    /// Acting user id (never nil after construction).
    pub fn user_id(&self) -> Uuid {
        self.user_id
    }

    /// Permission codes resolved for this membership (plain set until 1C).
    pub fn permissions(&self) -> &BTreeSet<String> {
        &self.permissions
    }

    /// Collection ids the actor may touch (plain set until full ACL in 1C).
    pub fn allowed_collection_ids(&self) -> &BTreeSet<Uuid> {
        &self.allowed_collection_ids
    }

    /// Whether the actor holds the named permission code.
    pub fn has_permission(&self, code: &str) -> bool {
        self.permissions.contains(code)
    }

    /// Whether the collection is in the allowed set (empty set means none allowed).
    pub fn allows_collection(&self, collection_id: Uuid) -> bool {
        self.allowed_collection_ids.contains(&collection_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_nil_org_and_user() {
        let user = Uuid::new_v4();
        let org = Uuid::new_v4();
        assert_eq!(
            OrgContext::try_new(Uuid::nil(), user, [] as [&str; 0], []),
            Err(OrgContextError::MissingOrgId)
        );
        assert_eq!(
            OrgContext::try_new(org, Uuid::nil(), [] as [&str; 0], []),
            Err(OrgContextError::MissingUserId)
        );
    }

    #[test]
    fn accepts_non_nil_scope() {
        let org = Uuid::new_v4();
        let user = Uuid::new_v4();
        let collection = Uuid::new_v4();
        let ctx = OrgContext::try_new(org, user, ["doc.upload"], [collection]).unwrap();
        assert_eq!(ctx.org_id(), org);
        assert_eq!(ctx.user_id(), user);
        assert!(ctx.has_permission("doc.upload"));
        assert!(ctx.allows_collection(collection));
    }
}

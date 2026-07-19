//! Conversion promotion identity and checkpoint helpers.

use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::jobs::CheckpointPayload;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversionIdentity {
    pub org_id: Uuid,
    pub document_id: Uuid,
    pub source_version_id: Uuid,
    pub job_idempotency_key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversionStep {
    Downloaded,
    Converted,
    StagingIntent,
    Staged,
    Promoted,
}

impl ConversionIdentity {
    pub fn new(
        org_id: Uuid,
        document_id: Uuid,
        source_version_id: Uuid,
        job_idempotency_key: impl Into<String>,
    ) -> Self {
        Self {
            org_id,
            document_id,
            source_version_id,
            job_idempotency_key: job_idempotency_key.into(),
        }
    }

    pub fn promoted_version_id(&self) -> Uuid {
        deterministic_uuid("markhand-conversion-promoted-version-v1", |hasher| {
            self.hash_material(hasher);
        })
    }

    pub fn markdown_artifact_id(&self) -> Uuid {
        deterministic_uuid("markhand-conversion-markdown-artifact-v1", |hasher| {
            self.hash_material(hasher);
        })
    }

    pub fn staged_markdown_object_id(&self, job_id: Uuid, attempts: i32) -> Uuid {
        deterministic_uuid("markhand-conversion-staged-markdown-attempt-v1", |hasher| {
            self.hash_material(hasher);
            hasher.update(job_id.as_bytes());
            hasher.update(attempts.to_be_bytes());
        })
    }

    pub fn storage_quota_reservation_key(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"markhand-conversion-storage-quota-v1");
        self.hash_material(&mut hasher);
        let digest = hex::encode(hasher.finalize());
        format!("convert.storage.{}", &digest[..48])
    }

    pub fn index_outbox_key(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"markhand-conversion-index-outbox-v1");
        self.hash_material(&mut hasher);
        let digest = hex::encode(hasher.finalize());
        format!("document.index_requested.{}", &digest[..48])
    }

    pub fn step_id(&self, step: ConversionStep) -> Uuid {
        deterministic_uuid("markhand-conversion-checkpoint-step-v1", |hasher| {
            self.hash_material(hasher);
            hasher.update(step.as_bytes());
        })
    }

    fn hash_material(&self, hasher: &mut Sha256) {
        hasher.update(self.org_id.as_bytes());
        hasher.update(self.document_id.as_bytes());
        hasher.update(self.source_version_id.as_bytes());
        hasher.update(self.job_idempotency_key.as_bytes());
        hasher.update(b"fileconv-sandbox-v1");
    }
}

impl ConversionStep {
    const fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::Downloaded => b"downloaded",
            Self::Converted => b"converted",
            Self::StagingIntent => b"staging_intent",
            Self::Staged => b"staged",
            Self::Promoted => b"promoted",
        }
    }
}

pub fn checkpoint_with_step(
    existing: Option<&JsonValue>,
    identity: &ConversionIdentity,
    step: ConversionStep,
) -> CheckpointPayload {
    let mut checkpoint = existing
        .cloned()
        .and_then(|value| serde_json::from_value::<CheckpointPayload>(value).ok())
        .unwrap_or_default();
    checkpoint.cursor_id = Some(identity.promoted_version_id());
    let step_id = identity.step_id(step);
    if !checkpoint.completed_ids.contains(&step_id) {
        checkpoint.completed_ids.push(step_id);
    }
    checkpoint
}

pub fn checkpoint_with_staged_key(
    existing: Option<&JsonValue>,
    identity: &ConversionIdentity,
    object_key: &str,
) -> CheckpointPayload {
    let mut checkpoint = checkpoint_with_step(existing, identity, ConversionStep::StagingIntent);
    if !checkpoint
        .staged_object_keys
        .iter()
        .any(|existing| existing == object_key)
    {
        checkpoint.staged_object_keys.push(object_key.to_string());
    }
    checkpoint
}

pub fn staged_keys_from_checkpoint(existing: Option<&JsonValue>) -> Vec<String> {
    existing
        .cloned()
        .and_then(|value| serde_json::from_value::<CheckpointPayload>(value).ok())
        .map(|checkpoint| checkpoint.staged_object_keys)
        .unwrap_or_default()
}

fn deterministic_uuid(label: &'static str, write: impl FnOnce(&mut Sha256)) -> Uuid {
    let mut hasher = Sha256::new();
    hasher.update(label.as_bytes());
    write(&mut hasher);
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    Uuid::from_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversion_identity_is_stable_and_distinct_by_source() {
        let org_id = Uuid::new_v4();
        let document_id = Uuid::new_v4();
        let source = Uuid::new_v4();
        let same = ConversionIdentity::new(org_id, document_id, source, "convert-a");
        let retry = ConversionIdentity::new(org_id, document_id, source, "convert-a");
        let other = ConversionIdentity::new(org_id, document_id, Uuid::new_v4(), "convert-a");
        assert_eq!(same.promoted_version_id(), retry.promoted_version_id());
        assert_ne!(same.promoted_version_id(), other.promoted_version_id());
        assert_ne!(
            same.step_id(ConversionStep::Downloaded),
            same.step_id(ConversionStep::Promoted)
        );
    }
}

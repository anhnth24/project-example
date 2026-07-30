//! Round-robin org rotation for multi-org worker processes (1C-10).
//!
//! `bin/worker.rs` historically pinned exactly one org per process
//! (`MARKHAND_WORKER_ORG_ID` = single UUID): every claim transaction sets the
//! RLS GUC to that org, so cross-org fairness could only be arranged by
//! deployment (one process fleet per org). [`OrgRotation`] lets one process
//! serve several explicitly configured orgs with a deterministic fairness
//! bound: each cycle scans orgs starting at the cursor, serves at most one
//! job, then moves the cursor past the org that was served. An org with
//! pending work is therefore attempted before any previously served org is
//! served again — between two consecutive jobs of one org, at most `N - 1`
//! jobs of other orgs run, no matter how large another org's backlog is.
//!
//! This deliberately reuses the existing per-org claim path (RLS +
//! `org_id = $1` + 1C-09 `concurrent_jobs` reservation) instead of a
//! cross-org claim query: `jobs` is FORCE-RLS and the claim SQL is
//! org-scoped by design, so a fair `ORDER BY` across orgs inside one claim
//! is unreachable without weakening the tenancy posture.

use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::auth::context::OrgContext;

#[derive(Debug)]
pub struct OrgRotation {
    contexts: Vec<OrgContext>,
    cursor: AtomicUsize,
}

impl OrgRotation {
    /// Builds a rotation over one or more org contexts.
    ///
    /// Rejects an empty list and duplicate org ids (a duplicate would grant
    /// that org two turns per rotation, silently breaking the fairness bound).
    pub fn new(contexts: Vec<OrgContext>) -> Result<Self, String> {
        if contexts.is_empty() {
            return Err("org rotation requires at least one org context".into());
        }
        for (index, ctx) in contexts.iter().enumerate() {
            if contexts[..index]
                .iter()
                .any(|earlier| earlier.org_id() == ctx.org_id())
            {
                return Err(format!(
                    "org rotation rejects duplicate org id {}",
                    ctx.org_id()
                ));
            }
        }
        Ok(Self {
            contexts,
            cursor: AtomicUsize::new(0),
        })
    }

    pub fn len(&self) -> usize {
        self.contexts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.contexts.is_empty()
    }

    pub fn contexts(&self) -> &[OrgContext] {
        &self.contexts
    }

    /// Runs one fair cycle: attempts each org in rotation order starting at
    /// the cursor and returns the first served outcome.
    ///
    /// `attempt` returns `Ok(Some(_))` when it served a job for that org,
    /// `Ok(None)` when the org had nothing claimable. After serving, the
    /// cursor advances past the served org so the next cycle starts with the
    /// following org. `Ok(None)` from every org means a fully idle cycle (the
    /// caller sleeps); the cursor is left unchanged. On `Err` the cursor also
    /// advances past the failing org so one poisoned org cannot pin the
    /// rotation head and starve the others across retry cycles.
    pub async fn run_cycle<T, E, F, Fut>(&self, mut attempt: F) -> Result<Option<T>, E>
    where
        F: FnMut(OrgContext) -> Fut,
        Fut: Future<Output = Result<Option<T>, E>>,
    {
        let n = self.contexts.len();
        let start = self.cursor.load(Ordering::Relaxed);
        for offset in 0..n {
            let index = (start + offset) % n;
            match attempt(self.contexts[index].clone()).await {
                Ok(Some(outcome)) => {
                    self.cursor.store((index + 1) % n, Ordering::Relaxed);
                    return Ok(Some(outcome));
                }
                Ok(None) => {}
                Err(error) => {
                    self.cursor.store((index + 1) % n, Ordering::Relaxed);
                    return Err(error);
                }
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use uuid::Uuid;

    fn ctx(org: u128) -> OrgContext {
        OrgContext::try_new(
            Uuid::from_u128(org),
            Uuid::from_u128(999),
            [] as [&str; 0],
            [],
        )
        .expect("ctx")
    }

    fn org_of(context: &OrgContext) -> Uuid {
        context.org_id()
    }

    #[test]
    fn rejects_empty_and_duplicate_orgs() {
        assert!(OrgRotation::new(Vec::new()).is_err());
        assert!(OrgRotation::new(vec![ctx(1), ctx(2), ctx(1)]).is_err());
        assert!(OrgRotation::new(vec![ctx(1)]).is_ok());
    }

    #[tokio::test]
    async fn huge_backlog_org_cannot_starve_sparse_org() {
        // Org A has "unbounded" backlog, org B has 3 jobs: services must
        // alternate A,B,A,B,A,B while B still has work (bound: at most one
        // A job between two consecutive B jobs), then continue with A only.
        let rotation = OrgRotation::new(vec![ctx(1), ctx(2)]).expect("rotation");
        let mut backlog: HashMap<Uuid, u32> = HashMap::new();
        backlog.insert(Uuid::from_u128(1), u32::MAX);
        backlog.insert(Uuid::from_u128(2), 3);
        let mut served = Vec::new();
        for _ in 0..10 {
            let outcome: Option<Uuid> = rotation
                .run_cycle(|context| {
                    let org = org_of(&context);
                    let remaining = backlog.get_mut(&org).copied().unwrap_or(0);
                    let claimed = remaining > 0;
                    if claimed {
                        *backlog.get_mut(&org).expect("org") -= 1;
                    }
                    async move { Ok::<_, String>(claimed.then_some(org)) }
                })
                .await
                .expect("cycle");
            served.push(outcome.expect("work available"));
        }
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        assert_eq!(served[..6], [a, b, a, b, a, b]);
        assert!(served[6..].iter().all(|org| *org == a));
    }

    #[tokio::test]
    async fn idle_orgs_are_skipped_within_one_cycle() {
        // Org A empty must not cost an idle cycle when org B has work.
        let rotation = OrgRotation::new(vec![ctx(1), ctx(2)]).expect("rotation");
        let outcome: Option<Uuid> = rotation
            .run_cycle(|context| {
                let org = org_of(&context);
                async move { Ok::<_, String>((org == Uuid::from_u128(2)).then_some(org)) }
            })
            .await
            .expect("cycle");
        assert_eq!(outcome, Some(Uuid::from_u128(2)));
        // Fully idle cycle returns None.
        let idle: Option<Uuid> = rotation
            .run_cycle(|_| async { Ok::<_, String>(None) })
            .await
            .expect("cycle");
        assert_eq!(idle, None);
    }

    #[tokio::test]
    async fn error_advances_cursor_past_poison_org() {
        let rotation = OrgRotation::new(vec![ctx(1), ctx(2)]).expect("rotation");
        let result: Result<Option<Uuid>, String> = rotation
            .run_cycle(|context| {
                let org = org_of(&context);
                async move {
                    if org == Uuid::from_u128(1) {
                        Err("boom".to_string())
                    } else {
                        Ok(Some(org))
                    }
                }
            })
            .await;
        assert!(result.is_err());
        // Next cycle must start at org 2, so the poison org cannot pin the head.
        let outcome: Option<Uuid> = rotation
            .run_cycle(|context| {
                let org = org_of(&context);
                async move {
                    assert_ne!(org, Uuid::from_u128(1), "cycle must start after poison org");
                    Ok::<_, String>(Some(org))
                }
            })
            .await
            .expect("cycle");
        assert_eq!(outcome, Some(Uuid::from_u128(2)));
    }
}

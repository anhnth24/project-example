//! Bounded blocking embedding executor used by index workers.
//!
//! Local embedding is CPU-bound. The semaphore provides explicit backpressure
//! before work enters `spawn_blocking`, keeping an index retry from creating an
//! unbounded blocking queue.

use std::sync::Arc;

use thiserror::Error;
use tokio::sync::Semaphore;

use crate::services::embedding::{self, EmbeddingError};

const DEFAULT_MAX_IN_FLIGHT_BATCHES: usize = 1;

#[derive(Debug, Clone)]
pub struct EmbeddingWorker {
    permits: Arc<Semaphore>,
}

impl Default for EmbeddingWorker {
    fn default() -> Self {
        Self::new()
    }
}

impl EmbeddingWorker {
    pub fn new() -> Self {
        Self::with_max_in_flight(DEFAULT_MAX_IN_FLIGHT_BATCHES)
            .expect("default embedding backpressure configuration is valid")
    }

    pub fn with_max_in_flight(max_in_flight: usize) -> Result<Self, EmbeddingWorkerError> {
        if max_in_flight == 0 {
            return Err(EmbeddingWorkerError::InvalidConcurrency);
        }
        Ok(Self {
            permits: Arc::new(Semaphore::new(max_in_flight)),
        })
    }

    /// Runs a validated local embedding batch outside Tokio's async executor.
    pub async fn embed_bodies(
        &self,
        bodies: Vec<String>,
    ) -> Result<Vec<Vec<f32>>, EmbeddingWorkerError> {
        let permit = self
            .permits
            .clone()
            .try_acquire_owned()
            .map_err(|_| EmbeddingWorkerError::Backpressure)?;
        let output = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            embedding::embed_bodies(&bodies)
        })
        .await
        .map_err(|_| EmbeddingWorkerError::Join)??;
        Ok(output)
    }
}

#[derive(Debug, Error)]
pub enum EmbeddingWorkerError {
    #[error("embedding worker has no available batch capacity")]
    Backpressure,
    #[error("embedding worker concurrency must be positive")]
    InvalidConcurrency,
    #[error("embedding task failed")]
    Join,
    #[error("embedding validation failed")]
    Embedding(#[from] EmbeddingError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_zero_concurrency() {
        assert!(matches!(
            EmbeddingWorker::with_max_in_flight(0),
            Err(EmbeddingWorkerError::InvalidConcurrency)
        ));
    }

    #[tokio::test]
    async fn rejects_a_batch_when_capacity_is_exhausted() {
        let worker = EmbeddingWorker::with_max_in_flight(1).unwrap();
        let permit = worker.permits.clone().try_acquire_owned().unwrap();

        assert!(matches!(
            worker.embed_bodies(vec!["nội dung".into()]).await,
            Err(EmbeddingWorkerError::Backpressure)
        ));
        drop(permit);
        assert_eq!(
            worker
                .embed_bodies(vec!["nội dung".into()])
                .await
                .unwrap()
                .len(),
            1
        );
    }
}

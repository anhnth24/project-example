//! Grounded Q&A engine over tenant-authorized retrieval hits.

pub mod grounding;
pub mod prompt;
pub mod provider;
pub mod stream;

pub use stream::{answer_question, QaAnswerMode, QaCitation, QaError, QaEvent, QaRequest};

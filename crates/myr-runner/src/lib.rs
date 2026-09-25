//! Mission execution and experimental harness. Fixture execution is explicitly
//! distinct from live-provider execution and official benchmark evidence.
pub mod acceptance;
pub mod accounting;
pub mod admission;
pub mod budget;
pub mod candidate;
pub mod collection;
pub mod command_verification;
pub mod context;
pub mod delta;
pub mod dispatch;
pub mod docker_sandbox;
pub mod goal;
pub mod measurement;
pub mod mission;
pub mod pilot;
pub mod pipeline;
pub mod planner;
pub mod planning;
pub mod prose;
pub mod review;
pub mod schedule;
pub mod setup;
pub mod snapshot;
pub mod source;
pub mod statistics;
pub mod task;
pub mod verifier;
pub mod views;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("runner I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("runner JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("runner validation: {0}")]
    Validation(#[from] myr_core::ValidationError),
    #[error("runner graph: {0}")]
    Graph(#[from] myr_graph::Error),
    #[error("runner CAS: {0}")]
    Cas(#[from] myr_cas::Error),
}
pub type Result<T> = std::result::Result<T, Error>;

//! Robotutor core: the deterministic teaching kernel.
//!
//! - `ledger`: append-only evidence and its projection (beliefs, memory, hypotheses)
//! - `atlas`: the shared knowledge release (nodes, item families, goal contracts)
//! - `policy`: deterministic choice of the next move
//! - `session`: the orchestrator state machine
//! - `ltl` + `spec`: LTLf properties checked against every trace
//! - `sim`: hidden-truth learners for calibrated, long-horizon simulation
//! - `llm` + `live`: model calls (renderer, graders, learner persona) through OpenRouter

pub mod atlas;
pub mod ledger;
pub mod llm;
pub mod live;
pub mod ltl;
pub mod policy;
pub mod session;
pub mod sim;
pub mod spec;

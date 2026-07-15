#![warn(missing_docs)]
//! Deterministic session-forensics core for suMCP.
//!
//! Pipeline: locate → ingest → model → signals → score → Report (SPEC §4).
//! This crate is synchronous and pure — no I/O below `ingest`, no async
//! runtime (ADR A2). Signals are pure functions `&Session -> Vec<Finding>`,
//! and every finding carries the action indices proving it.

pub mod model;

#![warn(missing_docs)]
//! Deterministic session-forensics core for suMCP.
//!
//! Pipeline: locate → ingest → model → signals → score → Report (SPEC §4).
//! This crate is synchronous and pure — no I/O below `ingest`, no async
//! runtime (ADR A2). Signals are pure functions `&Session -> Vec<Finding>`,
//! and every finding carries the action indices proving it.

pub mod ingest;
pub mod locate;
pub mod model;
pub mod report;
pub mod signals;

use std::path::Path;

/// Parse a transcript file into an [`report::Overview`].
///
/// Reads the file as the main lane. Returns an `io::Error` only if the file
/// cannot be read — parsing itself never fails (bad lines are counted).
pub fn overview_of_file(path: &Path) -> std::io::Result<report::Overview> {
    // `?` propagates the io::Error to the caller if the read fails; on success
    // it unwraps the String and execution continues.
    let raw = std::fs::read_to_string(path)?;
    let session = ingest::ingest_str(&raw, model::Lane::Main);
    Ok(report::Overview::from_session(&session))
}

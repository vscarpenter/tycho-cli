//! `tycho` turns Claude Code's local JSONL transcripts into usage analytics:
//! tokens, cost, cache economics, and trends by day, project, session, and
//! model.
//!
//! Privacy is structural: the deserialization types in [`record`] have no
//! field that could hold message content, so content can never be parsed,
//! displayed, exported, or persisted. The tool is read-only and makes no
//! network calls at runtime.

pub mod aggregate;
pub mod cache;
pub mod cli;
pub mod cost;
pub mod dedupe;
pub mod discover;
pub mod pricing;
pub mod record;
pub mod report;
pub mod scan;

/// The name of the installed binary. Kept in one place (plus the `[[bin]]`
/// stanza in `Cargo.toml`) so renaming the tool is a two-line change.
pub const BIN_NAME: &str = "tycho";

//! Report rendering: human tables, machine-readable JSON, and CSV.
//!
//! Renderers are pure functions from aggregates to `String`, so they are
//! trivially testable and the CLI owns all terminal concerns. No color is
//! used yet; when color arrives it must respect `NO_COLOR` and non-TTY
//! output.

pub mod csv;
pub mod json;
pub mod table;

//! Report rendering: human tables and machine-readable JSON.
//!
//! Renderers are pure functions from aggregates to `String`, so they are
//! trivially testable and the CLI owns all terminal concerns. Phase 1 uses
//! no color at all; when color arrives it must respect `NO_COLOR` and
//! non-TTY output.

pub mod json;
pub mod table;

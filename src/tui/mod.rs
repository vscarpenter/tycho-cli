//! `tycho live` — the ratatui dashboard (spec §5.2).
//!
//! [`state`] is the pure, testable core: a [`state::DashboardState`] derived
//! from `(events, mtimes, now, tz, pricing)` with an injected `now` so
//! time-relative math (burn rate, active session) is deterministic in tests.
//! Rendering and the terminal event loop arrive in later tasks.

pub mod state;

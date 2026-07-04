//! Dashboard UI state and key handling — the parts testable without a
//! terminal. Rendering lives in [`super::view`]; the event loop in
//! [`super::run`].

use ratatui::crossterm::event::{KeyCode, KeyModifiers};

use super::state::DashboardState;

/// Which tab is showing. `Tab` cycles with `Tab`/`BackTab`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    /// Headline totals, burn rate, cache gauge, active project.
    #[default]
    Overview,
    /// Today's sessions, active one first.
    Sessions,
    /// Today's per-model split.
    Models,
}

impl Tab {
    /// Tabs in display order.
    pub const ALL: [Tab; 3] = [Tab::Overview, Tab::Sessions, Tab::Models];

    /// Index into [`Tab::ALL`], for the ratatui `Tabs` widget.
    pub fn index(self) -> usize {
        Tab::ALL.iter().position(|&t| t == self).unwrap_or(0)
    }

    /// The next tab, wrapping.
    pub fn next(self) -> Tab {
        Tab::ALL[(self.index() + 1) % Tab::ALL.len()]
    }

    /// The previous tab, wrapping.
    pub fn prev(self) -> Tab {
        Tab::ALL[(self.index() + Tab::ALL.len() - 1) % Tab::ALL.len()]
    }
}

/// Whether the event loop should keep running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    /// Keep looping.
    Continue,
    /// Exit the dashboard.
    Quit,
}

/// Everything the UI thread holds between frames.
#[derive(Debug, Default)]
pub struct App {
    /// The current tab.
    pub tab: Tab,
    /// The latest snapshot, or `None` until the first scan lands.
    pub latest: Option<DashboardState>,
}

/// Translate a keypress into a control-flow decision, mutating `app`.
/// `q`/`Esc`/`Ctrl-C` quit; `Tab`/`Right` and `BackTab`/`Left` cycle tabs.
pub fn handle_key(code: KeyCode, mods: KeyModifiers, app: &mut App) -> Control {
    match code {
        KeyCode::Char('q') | KeyCode::Esc => Control::Quit,
        KeyCode::Char('c') if mods.contains(KeyModifiers::CONTROL) => Control::Quit,
        KeyCode::Tab | KeyCode::Right => {
            app.tab = app.tab.next();
            Control::Continue
        }
        KeyCode::BackTab | KeyCode::Left => {
            app.tab = app.tab.prev();
            Control::Continue
        }
        _ => Control::Continue,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn q_and_ctrl_c_quit() {
        let mut app = App::default();
        assert_eq!(
            handle_key(KeyCode::Char('q'), KeyModifiers::NONE, &mut app),
            Control::Quit
        );
        assert_eq!(
            handle_key(KeyCode::Char('c'), KeyModifiers::CONTROL, &mut app),
            Control::Quit
        );
        // plain 'c' does not quit
        assert_eq!(
            handle_key(KeyCode::Char('c'), KeyModifiers::NONE, &mut app),
            Control::Continue
        );
    }

    #[test]
    fn tab_cycles_forward_and_wraps() {
        let mut app = App::default();
        assert_eq!(app.tab, Tab::Overview);
        handle_key(KeyCode::Tab, KeyModifiers::NONE, &mut app);
        assert_eq!(app.tab, Tab::Sessions);
        handle_key(KeyCode::Tab, KeyModifiers::NONE, &mut app);
        assert_eq!(app.tab, Tab::Models);
        handle_key(KeyCode::Tab, KeyModifiers::NONE, &mut app);
        assert_eq!(app.tab, Tab::Overview);
    }

    #[test]
    fn backtab_cycles_backward() {
        let mut app = App::default();
        handle_key(KeyCode::BackTab, KeyModifiers::NONE, &mut app);
        assert_eq!(app.tab, Tab::Models);
    }
}

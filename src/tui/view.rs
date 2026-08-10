//! ratatui rendering of a [`DashboardState`] into a frame, one panel per tab.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Cell, Gauge, Paragraph, Row, Sparkline, Table, Tabs};
use rust_decimal::prelude::ToPrimitive;

use super::app::{App, Tab};
use super::state::DashboardState;

fn dollars(d: rust_decimal::Decimal) -> String {
    format!("${:.2}", d.to_f64().unwrap_or(0.0))
}

fn overview(frame: &mut Frame, area: Rect, state: &DashboardState) {
    let [head, gauge_area, spark_area] = Layout::vertical([
        Constraint::Length(6),
        Constraint::Length(3),
        Constraint::Min(3),
    ])
    .areas(area);

    let active = state
        .active
        .as_ref()
        .map(|a| format!("{} (idle {}s)", a.project, a.idle.num_seconds()))
        .unwrap_or_else(|| "idle".to_owned());
    let lines = vec![
        Line::from(format!(
            "Today   {}   {} tokens",
            dollars(state.today.actual_cost),
            state.today.totals.total()
        )),
        Line::from(format!(
            "Burn    {:.0} tok/min   {}/hr",
            state.burn.tokens_per_min,
            dollars(state.burn.cost_per_hour)
        )),
        Line::from(format!("Active  {active}")),
    ];
    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title("tycho live")),
        head,
    );

    let ratio = state
        .today
        .hit_rate
        .and_then(|r| r.to_f64())
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);
    frame.render_widget(
        Gauge::default()
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Cache hit rate"),
            )
            .ratio(ratio)
            .label(format!("{:.1}%", ratio * 100.0)),
        gauge_area,
    );

    frame.render_widget(
        Sparkline::default()
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Burn (10 min)"),
            )
            .data(state.burn.per_minute.iter().copied()),
        spark_area,
    );
}

fn models(frame: &mut Frame, area: Rect, state: &DashboardState) {
    let rows = state.models.iter().map(|m| {
        Row::new(vec![
            Cell::from(m.model.clone()),
            Cell::from(m.tokens.to_string()),
            Cell::from(dollars(m.cost)),
            Cell::from(
                m.hit_rate
                    .and_then(|r| r.to_f64())
                    .map(|r| format!("{:.1}%", r * 100.0))
                    .unwrap_or_else(|| "-".into()),
            ),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Min(20),
            Constraint::Length(12),
            Constraint::Length(10),
            Constraint::Length(8),
        ],
    )
    .header(Row::new(vec!["Model", "Tokens", "Cost", "Hit"]).style(Style::default().bold()))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title("Models (today)"),
    );
    frame.render_widget(table, area);
}

fn sessions(frame: &mut Frame, area: Rect, state: &DashboardState) {
    let rows = state.sessions.iter().map(|s| {
        let marker = if s.active { "●" } else { " " };
        Row::new(vec![
            Cell::from(marker),
            Cell::from(s.session_id.chars().take(8).collect::<String>()),
            Cell::from(s.project.clone()),
            Cell::from(s.tokens.to_string()),
            Cell::from(dollars(s.cost)),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(2),
            Constraint::Length(10),
            Constraint::Min(16),
            Constraint::Length(12),
            Constraint::Length(10),
        ],
    )
    .header(
        Row::new(vec!["", "Session", "Project", "Tokens", "Cost"]).style(Style::default().bold()),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title("Sessions (today)"),
    );
    frame.render_widget(table, area);
}

/// Draw the whole dashboard: a tab bar, the active panel, and a footer.
pub fn draw(frame: &mut Frame, app: &App) {
    let [tabs_area, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    let titles = Tab::ALL.iter().map(|t| match t {
        Tab::Overview => "Overview",
        Tab::Sessions => "Sessions",
        Tab::Models => "Models",
    });
    frame.render_widget(
        Tabs::new(titles)
            .select(app.tab.index())
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED)),
        tabs_area,
    );

    match &app.latest {
        None => frame.render_widget(Paragraph::new("Scanning…"), body),
        Some(state) => match app.tab {
            Tab::Overview => overview(frame, body, state),
            Tab::Sessions => sessions(frame, body, state),
            Tab::Models => models(frame, body, state),
        },
    }

    let footer_text = match &app.latest {
        Some(s) => format!(
            "updated {}  ·  q quit  ·  tab switch",
            s.generated_at.format("%H:%M:%S")
        ),
        None => "q quit  ·  tab switch".to_owned(),
    };
    frame.render_widget(
        Paragraph::new(footer_text).style(Style::default().dim()),
        footer,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn sample() -> DashboardState {
        use super::super::state::{ActiveSession, BurnRate, ModelLine, SessionLine};
        use crate::aggregate::Totals;
        let totals = Totals {
            input: 100,
            output: 200,
            cache_write_5m: 50,
            cache_write_1h: 10,
            cache_read: 1_000,
            cost: "0.64".parse().unwrap(),
        };
        DashboardState {
            today: crate::cache::CacheEconomics {
                model: "Total".into(),
                totals,
                hit_rate: Some("0.86".parse().unwrap()),
                actual_cost: "0.64".parse().unwrap(),
                counterfactual_cost: "1.08".parse().unwrap(),
                savings: "0.44".parse().unwrap(),
                leverage: Some("1.68".parse().unwrap()),
                ttl_premium: "0.02".parse().unwrap(),
            },
            burn: BurnRate {
                tokens_per_min: 12.0,
                cost_per_hour: "0.9".parse().unwrap(),
                per_minute: [1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
            },
            models: vec![ModelLine {
                model: "claude-opus-4-8".into(),
                tokens: 1_360,
                cost: "0.64".parse().unwrap(),
                hit_rate: Some("0.86".parse().unwrap()),
            }],
            sessions: vec![SessionLine {
                session_id: "sess-abcd".into(),
                project: "gsd".into(),
                tokens: 1_360,
                cost: "0.64".parse().unwrap(),
                last_activity: "2026-07-04T11:59:00Z".parse().unwrap(),
                active: true,
            }],
            active: Some(ActiveSession {
                project: "gsd".into(),
                idle: chrono::Duration::seconds(20),
            }),
            generated_at: "2026-07-04T12:00:00Z".parse().unwrap(),
        }
    }

    fn rendered(app: &App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    #[test]
    fn overview_shows_headline_and_active_project() {
        let text = rendered(&App {
            tab: Tab::Overview,
            latest: Some(sample()),
        });
        assert!(text.contains("Today"), "{text}");
        assert!(text.contains("gsd"), "{text}");
        assert!(text.contains("Cache hit rate"), "{text}");
    }

    #[test]
    fn models_tab_lists_the_model() {
        let text = rendered(&App {
            tab: Tab::Models,
            latest: Some(sample()),
        });
        assert!(text.contains("claude-opus-4-8"), "{text}");
    }

    #[test]
    fn no_snapshot_shows_scanning() {
        let text = rendered(&App::default());
        assert!(text.contains("Scanning"), "{text}");
    }
}

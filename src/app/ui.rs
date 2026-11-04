//! Small rendering helpers shared by screens.

use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType};
use std::time::{SystemTime, UNIX_EPOCH};

pub const ACCENT: Color = Color::Cyan;
pub const DIM: Style = Style::new().fg(Color::DarkGray);
pub const SELECTED: Style = Style::new().bg(Color::Rgb(45, 50, 65));

pub fn block(title: impl Into<String>, focused: bool) -> Block<'static> {
    let b = Block::bordered().border_type(BorderType::Rounded).title(format!(" {} ", title.into()));
    if focused { b.border_style(Style::new().fg(ACCENT)) } else { b.border_style(DIM) }
}

pub fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let [area] = Layout::horizontal([Constraint::Length(w.min(area.width))]).flex(Flex::Center).areas(area);
    let [area] = Layout::vertical([Constraint::Length(h.min(area.height))]).flex(Flex::Center).areas(area);
    area
}

/// Check-rollup / check-run state as a one-cell glyph.
pub fn ci(state: Option<&str>) -> Span<'static> {
    match state {
        Some("SUCCESS" | "NEUTRAL" | "SKIPPED") => "✓".green(),
        Some("FAILURE" | "ERROR" | "TIMED_OUT" | "CANCELLED" | "STARTUP_FAILURE" | "ACTION_REQUIRED") => "✗".red(),
        Some(_) => "●".yellow(),
        None => " ".into(),
    }
}

pub fn decision(d: Option<&str>) -> Span<'static> {
    match d {
        Some("APPROVED") => "approved".green(),
        Some("CHANGES_REQUESTED") => "changes".red(),
        Some("REVIEW_REQUIRED") => "review".yellow(),
        _ => "".into(),
    }
}

/// A comment as a header line ("author · 3h · badge") plus its rendered body.
pub fn comment(
    author: &Option<crate::gh::model::Actor>,
    when: &str,
    badge: Span<'static>,
    body: &str,
    hl: &crate::highlight::Highlighter,
) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(vec![
        "▍".fg(ACCENT),
        login(author).bold(),
        format!(" · {} ", ago(when)).dark_gray(),
        badge,
    ])];
    lines.extend(crate::markdown::render(body, hl).into_iter().map(|mut l| {
        l.spans.insert(0, "▏ ".dark_gray());
        l
    }));
    lines.push(Line::default());
    lines
}

pub fn login(a: &Option<crate::gh::model::Actor>) -> String {
    a.as_ref().map_or_else(|| "ghost".into(), |a| a.login.clone())
}

/// "2026-11-04T10:00:00Z" -> "3h"
pub fn ago(ts: &str) -> String {
    let Some(t) = parse_ts(ts) else { return ts.to_string() };
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let s = (now - t).max(0);
    match s {
        0..60 => "now".into(),
        60..3600 => format!("{}m", s / 60),
        3600..86400 => format!("{}h", s / 3600),
        86400..2_592_000 => format!("{}d", s / 86400),
        _ => format!("{}mo", s / 2_592_000),
    }
}

fn parse_ts(ts: &str) -> Option<i64> {
    let n = |r: std::ops::Range<usize>| ts.get(r)?.parse::<i64>().ok();
    let (y, m, d) = (n(0..4)?, n(5..7)?, n(8..10)?);
    let (hh, mm, ss) = (n(11..13)?, n(14..16)?, n(17..19)?);
    // days_from_civil (Howard Hinnant)
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86400 + hh * 3600 + mm * 60 + ss)
}

#[cfg(test)]
mod tests {
    #[test]
    fn timestamps() {
        assert_eq!(super::parse_ts("1970-01-02T00:00:01Z"), Some(86401));
        assert_eq!(super::parse_ts("2024-03-01T00:00:00Z"), Some(1_709_251_200));
    }
}

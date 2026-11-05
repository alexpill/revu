//! Small rendering helpers shared by screens.

use ratatui::Frame;
use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Constraint, Flex, Layout, Position, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType};
use std::time::{SystemTime, UNIX_EPOCH};

pub const ACCENT: Color = Color::Cyan;
pub const DIM: Style = Style::new().fg(Color::DarkGray);
pub const SELECTED: Style = Style::new().bg(Color::Rgb(45, 50, 65));
/// Left bar of a comment body, in its author's color.
const BAR: &str = "▍ ";

pub fn block(title: impl Into<String>, focused: bool) -> Block<'static> {
    let title = title.into();
    let b = Block::bordered().border_type(BorderType::Rounded);
    let b = if title.is_empty() {
        b
    } else {
        b.title(format!(" {title} "))
    };
    if focused {
        b.border_style(Style::new().fg(ACCENT))
    } else {
        b.border_style(DIM)
    }
}

pub fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let [area] = Layout::horizontal([Constraint::Length(w.min(area.width))])
        .flex(Flex::Center)
        .areas(area);
    let [area] = Layout::vertical([Constraint::Length(h.min(area.height))])
        .flex(Flex::Center)
        .areas(area);
    area
}

/// Where each title lands when drawn by `Tabs` (one-space padding, empty divider).
pub fn tab_rects(titles: &[Line], area: Rect) -> Vec<Rect> {
    let mut x = area.x;
    titles
        .iter()
        .map(|t| {
            let w = t.width() as u16 + 2;
            let r = Rect::new(x, area.y, w, 1).intersection(area);
            x = x.saturating_add(w);
            r
        })
        .collect()
}

/// Neither side of a [`Split`] gets narrower than this.
const MIN_PANE: u16 = 16;

/// The border between two side-by-side panes: dragged with the mouse or nudged
/// with `<` `>`, as in tmux. It sizes one pane, the other takes the rest.
#[derive(Default)]
pub struct Split {
    /// The sized pane is the right one.
    right: bool,
    /// Its width once the user moved the border.
    width: Option<u16>,
    /// Both panes and the sized pane's width, as last laid out.
    area: Rect,
    cur: u16,
    hover: bool,
    drag: bool,
}

impl Split {
    pub fn right() -> Self {
        Self {
            right: true,
            ..Self::default()
        }
    }

    /// Lays `area` out as [left, right], the sized pane `default` wide until moved.
    pub fn areas(&mut self, area: Rect, default: u16) -> [Rect; 2] {
        self.area = area;
        self.cur = clamp_pane(self.width.unwrap_or(default), area.width);
        Layout::horizontal([Constraint::Length(self.left()), Constraint::Fill(1)]).areas(area)
    }

    /// Width of the left pane.
    fn left(&self) -> u16 {
        if self.right {
            self.area.width - self.cur
        } else {
            self.cur
        }
    }

    fn set_left(&mut self, left: u16) {
        let w = if self.right {
            self.area.width.saturating_sub(left)
        } else {
            left
        };
        self.cur = clamp_pane(w, self.area.width);
        self.width = Some(self.cur);
    }

    /// Moves the border `by` columns (negative: to the left), once laid out.
    pub fn nudge(&mut self, by: i16) {
        if self.area.is_empty() {
            return;
        }
        self.set_left(self.left().saturating_add_signed(by));
    }

    /// Either edge drawn there: the left pane's right one or the right pane's left one.
    fn on_border(&self, pos: Position) -> bool {
        let x = self.area.x + self.left();
        self.area.contains(pos) && (pos.x == x || pos.x + 1 == x)
    }

    /// Hover, grab, drag and release; true when the border reacted and needs a redraw.
    pub fn on_mouse(&mut self, m: MouseEvent) -> bool {
        let pos = Position::new(m.column, m.row);
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                self.drag = self.on_border(pos);
                self.drag
            }
            MouseEventKind::Drag(MouseButton::Left) if self.drag => {
                // The left pane's right edge follows the pointer.
                self.set_left((m.column + 1).saturating_sub(self.area.x));
                true
            }
            MouseEventKind::Up(MouseButton::Left) if self.drag => {
                self.drag = false;
                self.hover = self.on_border(pos);
                true
            }
            MouseEventKind::Moved => {
                let hover = self.on_border(pos);
                std::mem::replace(&mut self.hover, hover) != hover
            }
            _ => false,
        }
    }

    /// Highlights the border while hovered or dragged; call once both panes are drawn.
    pub fn draw(&self, f: &mut Frame) {
        if self.hover || self.drag {
            let x = (self.area.x + self.left()).saturating_sub(1);
            let edges = Rect::new(x, self.area.y, 2, self.area.height).intersection(self.area);
            f.buffer_mut().set_style(edges, Style::new().fg(Color::Yellow));
        }
    }
}

/// Keeps `MIN_PANE` columns on both sides of `total` when there is room.
fn clamp_pane(w: u16, total: u16) -> u16 {
    w.max(MIN_PANE).min(total.saturating_sub(MIN_PANE))
}

/// Index of the row under `pos` in a one-line-per-row view starting at row `offset`.
pub fn row_at(area: Rect, offset: usize, pos: Position) -> Option<usize> {
    area.contains(pos).then(|| offset + (pos.y - area.y) as usize)
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
    let who = login(author);
    let c = author_color(&who);
    let mut lines = vec![Line::from(vec![
        "▍".fg(c),
        format!(" {who} ").bg(c).black().bold(),
        format!(" · {} ", ago(when)).dark_gray(),
        badge,
    ])];
    lines.extend(crate::markdown::render(body, hl).into_iter().map(|mut l| {
        l.spans.insert(0, BAR.fg(c));
        l
    }));
    lines.push(Line::default());
    lines
}

/// Stable per-login color, so a change of speaker stands out in a thread.
fn author_color(login: &str) -> Color {
    const PALETTE: [Color; 6] = [
        Color::Cyan,
        Color::Magenta,
        Color::Yellow,
        Color::Blue,
        Color::Green,
        Color::LightRed,
    ];
    let h = login
        .bytes()
        .fold(0usize, |h, b| h.wrapping_mul(31).wrapping_add(b as usize));
    PALETTE[h % PALETTE.len()]
}

pub fn login(a: &Option<crate::gh::model::Actor>) -> String {
    a.as_ref().map_or_else(|| "ghost".into(), |a| a.login.clone())
}

/// "2026-11-04T10:00:00Z" -> "3h"
pub fn ago(ts: &str) -> String {
    let Some(t) = parse_ts(ts) else { return ts.to_string() };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
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

/// Word-wraps a styled line to `width` columns.
pub fn wrap(line: Line<'static>, width: usize) -> Vec<Line<'static>> {
    let width = width.max(10);
    let mut out = vec![Vec::new()];
    let mut used = 0;
    for span in line.spans {
        for word in span.content.split_inclusive(' ') {
            let w = word.chars().count();
            if used + w > width && used > 0 {
                out.push(Vec::new());
                used = 0;
            }
            // Words longer than a line are hard-split.
            let mut chars: Vec<char> = word.chars().collect();
            while chars.len() > width {
                let head: String = chars.drain(..width).collect();
                out.last_mut().unwrap().push(Span::styled(head, span.style));
                out.push(Vec::new());
            }
            used += chars.len();
            out.last_mut()
                .unwrap()
                .push(Span::styled(chars.into_iter().collect::<String>(), span.style));
        }
    }
    out.into_iter().map(Line::from).collect()
}

/// A faint full-width rule, to set the description apart from the timeline.
pub fn separator(width: usize) -> Line<'static> {
    Line::from("─".repeat(width)).style(DIM)
}

/// Wraps a line from [`comment`], repeating the author's bar on every row.
pub fn wrap_comment(mut line: Line<'static>, width: usize) -> Vec<Line<'static>> {
    if line.spans.first().is_none_or(|s| s.content != BAR) {
        return wrap(line, width);
    }
    let bar = line.spans.remove(0);
    let mut rows = wrap(line, width.saturating_sub(BAR.chars().count()));
    for r in &mut rows {
        r.spans.insert(0, bar.clone());
    }
    rows
}

#[cfg(test)]
mod tests {
    #[test]
    fn wraps_words() {
        let lines = super::wrap(ratatui::text::Line::from("aaaa bbbb cccc dddddddddddddddddddddd"), 10);
        let t: Vec<_> = lines.iter().map(|l| l.to_string()).collect();
        assert_eq!(t, vec!["aaaa bbbb ", "cccc ", "dddddddddd", "dddddddddd", "dd"]);
    }

    #[test]
    fn wrapped_comment_keeps_its_bar() {
        use ratatui::text::Line;
        let rows = super::wrap_comment(Line::from(vec![super::BAR.into(), "aaaa bbbb cccc".into()]), 12);
        let t: Vec<_> = rows.iter().map(|l| l.to_string()).collect();
        assert_eq!(t, vec!["▍ aaaa bbbb ", "▍ cccc"]);
    }

    #[test]
    fn timestamps() {
        assert_eq!(super::parse_ts("1970-01-02T00:00:01Z"), Some(86401));
        assert_eq!(super::parse_ts("2024-03-01T00:00:00Z"), Some(1_709_251_200));
    }

    #[test]
    fn split_drags_within_bounds() {
        use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        use ratatui::layout::Rect;
        let ev = |kind, column| MouseEvent {
            kind,
            column,
            row: 5,
            modifiers: KeyModifiers::NONE,
        };
        let area = Rect::new(10, 2, 100, 20);
        let mut s = super::Split::default();
        let [l, r] = s.areas(area, 30);
        assert_eq!((l.width, r.x), (30, 40));
        // A press off the border is left to the click handler.
        assert!(!s.on_mouse(ev(MouseEventKind::Down(MouseButton::Left), 41)));
        assert!(!s.on_mouse(ev(MouseEventKind::Drag(MouseButton::Left), 60)));
        assert_eq!(s.areas(area, 30)[0].width, 30);
        // Both edges of the border grab it; the left pane's edge follows the pointer.
        assert!(s.on_mouse(ev(MouseEventKind::Moved, 39)));
        assert!(!s.on_mouse(ev(MouseEventKind::Moved, 40)), "still hovering");
        assert!(s.on_mouse(ev(MouseEventKind::Down(MouseButton::Left), 40)));
        assert!(s.on_mouse(ev(MouseEventKind::Drag(MouseButton::Left), 59)));
        assert_eq!(s.areas(area, 30)[0].width, 50);
        // Neither pane goes below MIN_PANE.
        s.on_mouse(ev(MouseEventKind::Drag(MouseButton::Left), 0));
        assert_eq!(s.areas(area, 30)[0].width, 16);
        s.on_mouse(ev(MouseEventKind::Drag(MouseButton::Left), 200));
        assert_eq!(s.areas(area, 30)[1].width, 16);
        assert!(s.on_mouse(ev(MouseEventKind::Up(MouseButton::Left), 200)));
        assert!(!s.on_mouse(ev(MouseEventKind::Drag(MouseButton::Left), 50)));

        // A right-sized pane keeps its width when the terminal grows.
        let mut s = super::Split::right();
        assert_eq!(s.areas(area, 38)[1].width, 38);
        s.nudge(-2);
        s.nudge(-2);
        assert_eq!(s.areas(Rect::new(0, 0, 200, 20), 38)[1].width, 42);
    }

    #[test]
    fn tab_rects_match_tabs_widget() {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;
        use ratatui::text::Line;
        use ratatui::widgets::{Tabs, Widget};
        let titles = vec![Line::from(" 1 home "), Line::from(" 2 a/b 12 "), Line::from(" 3 x ")];
        let area = Rect::new(3, 1, 40, 1);
        let mut buf = Buffer::empty(Rect::new(0, 0, 50, 2));
        Tabs::new(titles.clone()).select(1).divider("").render(area, &mut buf);
        let rects = super::tab_rects(&titles, area);
        let text = |r: Rect| (r.x..r.right()).map(|x| buf[(x, r.y)].symbol()).collect::<String>();
        assert_eq!(text(rects[0]), "  1 home  ");
        assert_eq!(text(rects[1]), "  2 a/b 12  ");
        assert_eq!(text(rects[2]), "  3 x  ");
    }
}

//! The `?` popup: the current screen's keys, filtered as you type.
//!
//! Lines are (key, description); an empty key makes the description a section title.

use super::ui;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Margin, Rect};
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};

pub struct Help {
    lines: Vec<(String, String)>,
    query: String,
    scroll: u16,
}

impl Help {
    pub fn new(lines: Vec<(String, String)>) -> Self {
        Self {
            lines,
            query: String::new(),
            scroll: 0,
        }
    }

    /// False once the popup should close.
    pub fn on_key(&mut self, k: KeyEvent) -> bool {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('?') => return false,
            KeyCode::Down => self.scroll = self.scroll.saturating_add(1),
            KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::PageDown => self.scroll = self.scroll.saturating_add(10),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::Char('u') if ctrl => self.query.clear(),
            KeyCode::Char(c) if !ctrl => self.query.push(c),
            KeyCode::Backspace => _ = self.query.pop(),
            _ => return true,
        }
        // A new query starts from the top.
        if !matches!(
            k.code,
            KeyCode::Down | KeyCode::Up | KeyCode::PageDown | KeyCode::PageUp
        ) {
            self.scroll = 0;
        }
        true
    }

    pub fn render(&mut self, f: &mut Frame) {
        // Sized on the whole list, so the popup stays put while filtering.
        let full = rows(&filter(&self.lines, "")).len() as u16;
        let area = ui::centered(f.area(), 80, full + 4);
        let rows = rows(&filter(&self.lines, &self.query));
        let block = ui::block("keys", true).title_bottom(
            Line::from(" type to filter · ↑↓ PgUp/PgDn scroll · Esc close ")
                .dark_gray()
                .right_aligned(),
        );
        let inner = block.inner(area);
        let [search, body] = [
            Rect { height: 1, ..inner },
            Rect {
                y: inner.y + 2,
                height: inner.height.saturating_sub(2),
                ..inner
            },
        ];
        f.render_widget(Clear, area);
        f.render_widget(block, area);
        let search_line = match self.query.is_empty() {
            true => Line::from(" / type to filter by key or description").dark_gray(),
            false => Line::from(vec![" / ".dark_gray(), Span::from(self.query.as_str()), "▏".cyan()]),
        };
        f.render_widget(search_line, search);
        if rows.is_empty() {
            f.render_widget(Line::from(" no matching keys").dark_gray(), body);
            return;
        }
        let max = (rows.len() as u16).saturating_sub(body.height);
        self.scroll = self.scroll.min(max);
        f.render_widget(Paragraph::new(rows).scroll((self.scroll, 0)), body);
        if max > 0 {
            let mut state = ScrollbarState::new(max as usize + 1)
                .position(self.scroll as usize)
                .viewport_content_length(body.height as usize);
            let bar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None);
            f.render_stateful_widget(bar, area.inner(Margin::new(0, 1)), &mut state);
        }
    }
}

/// Lines whose key or description contains `query` (any case), under their section titles.
fn filter<'a>(lines: &'a [(String, String)], query: &str) -> Vec<&'a (String, String)> {
    let q = query.to_lowercase();
    let mut out = Vec::new();
    let mut section = None;
    for l in lines {
        if l.0.is_empty() {
            section = Some(l);
        } else if l.0.to_lowercase().contains(&q) || l.1.to_lowercase().contains(&q) {
            out.extend(section.take());
            out.push(l);
        }
    }
    out
}

fn rows(lines: &[&(String, String)]) -> Vec<Line<'static>> {
    let w = lines.iter().map(|(k, _)| k.chars().count()).max().unwrap_or(0);
    let mut out = Vec::new();
    for (k, d) in lines {
        if k.is_empty() {
            if !out.is_empty() {
                out.push(Line::default());
            }
            out.push(Line::from(format!(" {d}")).bold().yellow());
        } else {
            out.push(Line::from(vec![
                format!("   {k:<w$}  ").cyan().bold(),
                Span::from(d.clone()),
            ]));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn filters_keys_and_descriptions_under_their_section() {
        let lines: Vec<(String, String)> = [
            ("", "move"),
            ("j/k", "scroll"),
            ("[ ]", "previous / next file"),
            ("", "review"),
            ("c", "comment line"),
            ("S", "submit review"),
        ]
        .iter()
        .map(|&(k, d)| (k.into(), d.into()))
        .collect();
        let keys = |q| {
            super::filter(&lines, q)
                .iter()
                .map(|l| l.0.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(keys(""), ["", "j/k", "[ ]", "", "c", "S"]);
        // Any case, on the description; the empty "move" section goes away.
        assert_eq!(keys("COMMENT"), ["", "c"]);
        // On the key.
        assert_eq!(keys("j/"), ["", "j/k"]);
        // Section titles alone don't match.
        assert_eq!(keys("move"), Vec::<&str>::new());
    }
}

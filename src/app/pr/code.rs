//! Read-only code buffers: any file of the PR head (or outside it, for
//! language-server jumps into dependencies), with search and LSP motions.

use super::Outcome;
use super::files::{with_cursor, word_motion};
use crate::app::ui::{self, DIM, SELECTED};
use crate::highlight::{Highlighter, StyledLine};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

pub struct Buffer {
    /// Repo-relative, or absolute for files outside the repo.
    pub path: String,
    /// Raw lines; `None` while loading.
    lines: Option<Result<Vec<String>, String>>,
    hl: Vec<StyledLine>,
    pub cursor: usize,
    /// Display column (tabs expanded).
    pub col: usize,
    scroll: usize,
    height: usize,
    pending_g: bool,
    /// `/` prompt being typed, and the last search.
    input: Option<String>,
    search: String,
}

impl Buffer {
    pub fn loading(path: String, cursor: usize, col: usize) -> Self {
        Self {
            path,
            lines: None,
            hl: Vec::new(),
            cursor,
            col,
            scroll: 0,
            height: 20,
            pending_g: false,
            input: None,
            search: String::new(),
        }
    }

    pub fn loaded(&mut self, text: Result<String, String>, hl: &Highlighter) {
        self.lines = Some(text.map(|t| t.lines().map(String::from).collect()));
        if let Some(Ok(lines)) = &self.lines {
            self.hl = hl.file(&self.path, lines.iter().map(String::as_str));
            // Center the target line once.
            self.scroll = self.cursor.saturating_sub(self.height / 3);
        }
    }

    pub fn goto(&mut self, line: usize, col: usize) {
        self.cursor = line;
        self.col = col;
        self.scroll = line.saturating_sub(self.height / 3);
    }

    pub fn raw_line(&self, n: usize) -> Option<&str> {
        self.lines.as_ref()?.as_ref().ok()?.get(n).map(String::as_str)
    }

    fn len(&self) -> usize {
        self.lines.as_ref().and_then(|l| l.as_ref().ok()).map_or(0, Vec::len)
    }

    fn chars(&self) -> Vec<char> {
        self.raw_line(self.cursor)
            .map(|l| l.replace('\t', "    ").chars().collect())
            .unwrap_or_default()
    }

    fn move_to(&mut self, line: usize) {
        self.cursor = line.min(self.len().saturating_sub(1));
    }

    /// Next (or previous) line containing the search, case-insensitive, wrapping.
    fn find(&mut self, forward: bool) {
        let needle = self.search.to_lowercase();
        let n = self.len();
        if needle.is_empty() || n == 0 {
            return;
        }
        let hit = (1..=n)
            .map(|o| {
                if forward {
                    (self.cursor + o) % n
                } else {
                    (self.cursor + n - o) % n
                }
            })
            .find(|&i| self.raw_line(i).is_some_and(|l| l.to_lowercase().contains(&needle)));
        if let Some(i) = hit {
            let line = self.raw_line(i).unwrap().replace('\t', "    ").to_lowercase();
            let col = line.find(&needle).map_or(0, |b| line[..b].chars().count());
            self.goto(i, col);
        }
    }

    pub fn on_key(&mut self, k: KeyEvent) -> Outcome {
        if let Some(input) = &mut self.input {
            match k.code {
                KeyCode::Esc => self.input = None,
                KeyCode::Enter => {
                    self.search = self.input.take().unwrap_or_default();
                    self.find(true);
                }
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Char(c) => input.push(c),
                _ => {}
            }
            return Outcome::Handled;
        }
        let pos = || Some((self.path.clone(), self.cursor as u32, self.col));
        if std::mem::take(&mut self.pending_g) {
            return match k.code {
                KeyCode::Char('g') => {
                    self.move_to(0);
                    Outcome::Handled
                }
                KeyCode::Char('d') => pos().map_or(Outcome::Handled, |(p, l, c)| Outcome::Definition(p, l, c)),
                KeyCode::Char('r') => pos().map_or(Outcome::Handled, |(p, l, c)| Outcome::References(p, l, c)),
                _ => Outcome::Handled,
            };
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Char('j') | KeyCode::Down => self.move_to(self.cursor + 1),
            KeyCode::Char('k') | KeyCode::Up => self.move_to(self.cursor.saturating_sub(1)),
            KeyCode::Char('d') if ctrl => self.move_to(self.cursor + self.height / 2),
            KeyCode::Char('u') if ctrl => self.move_to(self.cursor.saturating_sub(self.height / 2)),
            KeyCode::Char('h') | KeyCode::Left => self.col = self.col.saturating_sub(1),
            KeyCode::Char('l') | KeyCode::Right => self.col = (self.col + 1).min(self.chars().len().saturating_sub(1)),
            KeyCode::Char('w') => self.col = word_motion(&self.chars(), self.col, true),
            KeyCode::Char('b') => self.col = word_motion(&self.chars(), self.col, false),
            KeyCode::Char('0') => self.col = 0,
            KeyCode::Char('$') => self.col = self.chars().len().saturating_sub(1),
            KeyCode::Char('g') => self.pending_g = true,
            KeyCode::Char('G') => self.move_to(usize::MAX),
            KeyCode::Char('/') => self.input = Some(String::new()),
            KeyCode::Char('n') => self.find(true),
            KeyCode::Char('N') => self.find(false),
            KeyCode::Char('K') => return pos().map_or(Outcome::Handled, |(p, l, c)| Outcome::Hover(p, l, c)),
            _ => return Outcome::Unhandled,
        }
        Outcome::Handled
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        let block = ui::block(&self.path, true);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let lines = match &self.lines {
            None => return f.render_widget(Paragraph::new(" loading…").dark_gray(), inner),
            Some(Err(e)) => return f.render_widget(Paragraph::new(format!(" {e}")).red(), inner),
            Some(Ok(l)) => l,
        };
        let (code_area, prompt) = match &self.input {
            Some(_) => {
                let [a, b] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(inner);
                (a, Some(b))
            }
            None => (inner, None),
        };
        self.height = code_area.height as usize;
        self.cursor = self.cursor.min(lines.len().saturating_sub(1));
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.cursor >= self.scroll + self.height {
            self.scroll = self.cursor + 1 - self.height;
        }
        let width = lines.len().to_string().len().max(3);
        let rendered: Vec<Line> = self
            .hl
            .iter()
            .enumerate()
            .skip(self.scroll)
            .take(self.height)
            .map(|(i, code)| {
                let cur = i == self.cursor;
                let mut spans = vec![Span::styled(
                    format!("{:>width$} ", i + 1),
                    if cur { ui::ACCENT.into() } else { DIM },
                )];
                spans.extend(if cur { with_cursor(code, self.col) } else { code.clone() });
                let line = Line::from(spans);
                if cur { line.patch_style(SELECTED) } else { line }
            })
            .collect();
        f.render_widget(Paragraph::new(rendered), code_area);
        if let (Some(p), Some(input)) = (prompt, &self.input) {
            f.render_widget(
                Paragraph::new(Line::from(vec!["/".cyan(), input.clone().into(), "█".dark_gray()])),
                p,
            );
        }
    }
}

//! Fuzzy picker popup (file finder, LSP locations).

use super::ui::{self, SELECTED};
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::{Clear, List, ListState, Paragraph};

pub enum Picked {
    Pending,
    Cancel,
    Pick(usize),
}

pub struct Picker {
    title: String,
    items: Vec<String>,
    query: String,
    /// Indices into `items`, best match first.
    matches: Vec<usize>,
    state: ListState,
    matcher: Matcher,
}

impl Picker {
    pub fn new(title: impl Into<String>, items: Vec<String>) -> Self {
        let matches = (0..items.len()).collect();
        let matcher = Matcher::new(Config::DEFAULT.match_paths());
        Self { title: title.into(), items, query: String::new(), matches, state: ListState::default().with_selected(Some(0)), matcher }
    }

    fn refilter(&mut self) {
        let pattern = Pattern::parse(&self.query, CaseMatching::Smart, Normalization::Smart);
        let mut buf = Vec::new();
        let mut scored: Vec<(u32, usize)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, s)| Some((pattern.score(Utf32Str::new(s, &mut buf), &mut self.matcher)?, i)))
            .collect();
        // Best score first, shorter paths win ties.
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(self.items[a.1].len().cmp(&self.items[b.1].len())));
        self.matches = scored.into_iter().map(|(_, i)| i).collect();
        self.state.select(Some(0));
    }

    pub fn on_key(&mut self, k: KeyEvent) -> Picked {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let sel = self.state.selected().unwrap_or(0);
        match k.code {
            KeyCode::Esc => return Picked::Cancel,
            KeyCode::Enter => return self.matches.get(sel).map_or(Picked::Cancel, |&i| Picked::Pick(i)),
            KeyCode::Down | KeyCode::Tab => self.state.select(Some((sel + 1).min(self.matches.len().saturating_sub(1)))),
            KeyCode::Char('n' | 'j') if ctrl => self.state.select(Some((sel + 1).min(self.matches.len().saturating_sub(1)))),
            KeyCode::Up | KeyCode::BackTab => self.state.select(Some(sel.saturating_sub(1))),
            KeyCode::Char('p' | 'k') if ctrl => self.state.select(Some(sel.saturating_sub(1))),
            KeyCode::Char('u') if ctrl => {
                self.query.clear();
                self.refilter();
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.refilter();
            }
            KeyCode::Char(c) => {
                self.query.push(c);
                self.refilter();
            }
            _ => {}
        }
        Picked::Pending
    }

    pub fn render(&mut self, f: &mut Frame) {
        let area = ui::centered(f.area(), f.area().width * 7 / 10, f.area().height * 7 / 10);
        f.render_widget(Clear, area);
        let title = format!("{} {}/{}", self.title, self.matches.len(), self.items.len());
        let block = ui::block(title, true);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let [input, list] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(inner);
        f.render_widget(Paragraph::new(Line::from(vec!["› ".cyan(), self.query.clone().into(), "█".dark_gray()])), input);
        // Only materialize what fits on screen.
        let offset = self.state.selected().unwrap_or(0).saturating_sub((list.height as usize).saturating_sub(1));
        let items: Vec<Line> = self.matches.iter().skip(offset).take(list.height as usize).map(|&i| Line::from(self.items[i].clone())).collect();
        let mut state = ListState::default().with_selected(self.state.selected().map(|s| s - offset));
        f.render_stateful_widget(List::new(items).highlight_style(SELECTED), list, &mut state);
    }
}

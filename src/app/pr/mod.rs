//! A pull request: conversation, changed files and code buffers as tabs.

mod conversation;

use super::ui;
use super::{Action, Ctx, Key, Msg};
use crate::gh::{self, model::PrDetail, mutation::Mutation};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Tabs};

pub const HELP: &[(&str, &str)] = &[
    ("1 2 H/L", "conversation / files tabs"),
    ("j/k Ctrl-d/u g/G", "scroll"),
    ("C", "comment on the PR ($EDITOR)"),
    ("o", "open in browser"),
    ("Ctrl-r", "refresh"),
    ("q / Esc", "back"),
];

pub struct PrScreen {
    key: Key,
    detail: Option<Result<PrDetail, String>>,
    tab: usize,
    conv_scroll: u16,
}

impl PrScreen {
    pub fn open(key: Key, ctx: &mut Ctx) -> Self {
        let s = Self { key, detail: None, tab: 0, conv_scroll: 0 };
        s.load(ctx);
        // Bring the PR head into the local clone so file reads stay local.
        if ctx.sources[key.0].local.is_some() {
            let src = ctx.sources[key.0].clone();
            ctx.spawn(move || Msg::Fetched(key, src.fetch_pr(key.1)));
        }
        s
    }

    fn load(&self, ctx: &mut Ctx) {
        let key = self.key;
        let (owner, name) = ctx.cfg.repos[key.0].owner_name();
        let (owner, name) = (owner.to_string(), name.to_string());
        ctx.spawn(move || Msg::Pr(key, gh::pr(&owner, &name, key.1)));
    }

    fn detail(&self) -> Option<&PrDetail> {
        self.detail.as_ref()?.as_ref().ok()
    }

    pub fn on_msg(&mut self, msg: Msg, ctx: &mut Ctx) -> Option<Msg> {
        match msg {
            Msg::Pr(key, r) if key == self.key => self.detail = Some(r.map_err(|e| e.to_string())),
            Msg::Fetched(key, r) if key == self.key => {
                if let Err(e) = r {
                    ctx.error(format!("local fetch failed, using the API: {e}"));
                }
            }
            Msg::Mutated(key, m, _) if key == self.key => {
                if m.needs_reload() {
                    self.load(ctx);
                }
            }
            m => return Some(m),
        }
        None
    }

    pub fn on_key(&mut self, k: KeyEvent, ctx: &mut Ctx) -> Action {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Char('q') | KeyCode::Esc => return Action::Pop,
            KeyCode::Char('?') => return Action::Help,
            KeyCode::Char('r') if ctrl => self.load(ctx),
            KeyCode::Char('o') => ctx.browse(self.key),
            KeyCode::Char('1') => self.tab = 0,
            KeyCode::Char('C') => {
                if let Some(d) = self.detail() {
                    let id = d.id.clone();
                    ctx.compose(self.key, &id, Mutation::Comment { body: String::new() });
                }
            }
            KeyCode::Char('d') if ctrl => self.conv_scroll += 15,
            KeyCode::Char('u') if ctrl => self.conv_scroll = self.conv_scroll.saturating_sub(15),
            KeyCode::Char('j') | KeyCode::Down => self.conv_scroll += 1,
            KeyCode::Char('k') | KeyCode::Up => self.conv_scroll = self.conv_scroll.saturating_sub(1),
            KeyCode::Char('g') => self.conv_scroll = 0,
            KeyCode::Char('G') => self.conv_scroll = u16::MAX / 2,
            _ => {}
        }
        Action::None
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let [head, tabs_area, body] =
            Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Fill(1)]).areas(area);
        let repo = ctx.repo(self.key.0).to_string();
        let d = match &self.detail {
            None => return f.render_widget(Paragraph::new(format!(" {repo} #{} loading…", self.key.1)).dark_gray(), head),
            Some(Err(e)) => return f.render_widget(Paragraph::new(format!(" {e}")).red(), head),
            Some(Ok(d)) => d,
        };
        let pending = d.pending_comments();
        let mut title = vec![
            " ".into(),
            conversation::state_badge(d),
            format!(" {repo} #{} ", d.number).dark_gray(),
            d.title.clone().bold(),
        ];
        if pending > 0 {
            title.push(format!("  {pending} pending comment(s), S to submit").yellow());
        }
        f.render_widget(Paragraph::new(Line::from(title)), head);

        let titles = vec![Line::from(" 1 conversation ")];
        let tabs = Tabs::new(titles).select(self.tab).divider("").highlight_style(Style::new().fg(ui::ACCENT).bold().reversed());
        f.render_widget(tabs, tabs_area);

        self.conv_scroll = conversation::render(f, body, d, &ctx.hl, self.conv_scroll);
    }
}

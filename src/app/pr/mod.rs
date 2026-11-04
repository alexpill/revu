//! A pull request: conversation, changed files and code buffers as tabs.

mod conversation;
mod files;

use super::ui;
use super::{Action, Ctx, Key, Msg};
use crate::gh::{self, model::PrDetail, mutation::Mutation};
use files::{FilesView, Outcome};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{Clear, Paragraph, Tabs};

pub const HELP: &[(&str, &str)] = &[
    ("1 2 H/L", "conversation / files tabs"),
    ("j/k Ctrl-d/u g/G", "scroll / move"),
    ("Tab", "files: switch list / diff"),
    ("[ ]", "previous / next file"),
    ("{ }", "previous / next hunk"),
    ("Space", "toggle viewed (and go to next unviewed)"),
    ("h/l w/b 0/$", "move in line"),
    ("v", "start / cancel range selection"),
    ("c", "comment line or range (pending review)"),
    ("r / R", "on a thread: reply / resolve toggle"),
    ("z", "unfold generated file"),
    ("S", "submit review: comment / approve / request changes"),
    ("C", "comment on the PR"),
    ("o", "open in browser"),
    ("Ctrl-r", "refresh"),
    ("q / Esc", "back"),
    ("", "in $EDITOR, :cq cancels"),
];

const SUBMIT: &[(char, &str, Option<&str>)] = &[
    ('c', "comment", Some("COMMENT")),
    ('a', "approve", Some("APPROVE")),
    ('x', "request changes", Some("REQUEST_CHANGES")),
    ('d', "discard pending review", None),
];

pub struct PrScreen {
    key: Key,
    detail: Option<Result<PrDetail, String>>,
    files: Option<Result<FilesView, String>>,
    tab: usize,
    conv_scroll: u16,
    submit_menu: bool,
}

impl PrScreen {
    pub fn open(key: Key, ctx: &mut Ctx) -> Self {
        let s = Self { key, detail: None, files: None, tab: 0, conv_scroll: 0, submit_menu: false };
        s.load(ctx);
        s.load_files(ctx);
        // Bring the PR head into the local clone so file reads stay local.
        if ctx.sources[key.0].local.is_some() {
            let src = ctx.sources[key.0].clone();
            ctx.spawn(move || Msg::Fetched(key, src.fetch_pr(key.1)));
        }
        s
    }

    fn owner_name(&self, ctx: &Ctx) -> (String, String) {
        let (o, n) = ctx.cfg.repos[self.key.0].owner_name();
        (o.to_string(), n.to_string())
    }

    fn load(&self, ctx: &mut Ctx) {
        let key = self.key;
        let (owner, name) = self.owner_name(ctx);
        ctx.spawn(move || Msg::Pr(key, gh::pr(&owner, &name, key.1)));
    }

    fn load_files(&self, ctx: &mut Ctx) {
        let key = self.key;
        let (owner, name) = self.owner_name(ctx);
        ctx.spawn(move || Msg::Files(key, gh::pr_files(&owner, &name, key.1)));
    }

    fn detail(&self) -> Option<&PrDetail> {
        self.detail.as_ref()?.as_ref().ok()
    }

    pub fn on_msg(&mut self, msg: Msg, ctx: &mut Ctx) -> Option<Msg> {
        match msg {
            Msg::Pr(key, r) if key == self.key => self.detail = Some(r.map_err(|e| e.to_string())),
            Msg::Files(key, r) if key == self.key => match (r, &mut self.files) {
                (Ok((files, viewed)), Some(Ok(view))) => view.replace(files::entries(files, &viewed)),
                (Ok((files, viewed)), slot) => *slot = Some(Ok(FilesView::new(files::entries(files, &viewed)))),
                (Err(e), slot) => *slot = Some(Err(e.to_string())),
            },
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
        if self.submit_menu {
            self.submit_menu = false;
            if let (KeyCode::Char(c), Some(d)) = (k.code, self.detail()) {
                let id = d.id.clone();
                match SUBMIT.iter().find(|(key, ..)| *key == c) {
                    Some((_, _, Some(event))) => ctx.compose(self.key, &id, Mutation::Submit { event, body: String::new() }),
                    Some((_, _, None)) => ctx.mutate(self.key, &id, Mutation::DiscardReview),
                    None => {}
                }
            }
            return Action::None;
        }
        if self.tab == 1 {
            if let Some(Ok(view)) = &mut self.files {
                let detail = self.detail.as_ref().and_then(|d| d.as_ref().ok());
                match view.on_key(k, ctx, self.key, detail) {
                    Outcome::Handled => return Action::None,
                    Outcome::Unhandled => {}
                    // Wired to the language server in a later step.
                    Outcome::Definition(..) | Outcome::References(..) | Outcome::Hover(..) => {
                        ctx.info("no language server yet");
                        return Action::None;
                    }
                }
            }
        }

        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Char('q') | KeyCode::Esc => return Action::Pop,
            KeyCode::Char('?') => return Action::Help,
            KeyCode::Char('r') if ctrl => {
                self.load(ctx);
                self.load_files(ctx);
            }
            KeyCode::Char('o') => ctx.browse(self.key),
            KeyCode::Char('1') => self.tab = 0,
            KeyCode::Char('2') => self.tab = 1,
            KeyCode::Char('H') | KeyCode::Char('L') => self.tab = 1 - self.tab,
            KeyCode::Char('S') => self.submit_menu = self.detail().is_some(),
            KeyCode::Char('C') => {
                if let Some(d) = self.detail() {
                    let id = d.id.clone();
                    ctx.compose(self.key, &id, Mutation::Comment { body: String::new() });
                }
            }
            _ if self.tab != 0 => {}
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

        let files_title = match &self.files {
            Some(Ok(v)) => format!(" 2 files {}/{} ", v.viewed_count(), v.files.len()),
            _ => " 2 files ".into(),
        };
        let titles = vec![Line::from(" 1 conversation "), Line::from(files_title)];
        let tabs = Tabs::new(titles).select(self.tab).divider("").highlight_style(Style::new().fg(ui::ACCENT).bold().reversed());
        f.render_widget(tabs, tabs_area);

        match self.tab {
            0 => self.conv_scroll = conversation::render(f, body, d, &ctx.hl, self.conv_scroll),
            _ => match &mut self.files {
                None => f.render_widget(Paragraph::new(" loading files…").dark_gray(), body),
                Some(Err(e)) => f.render_widget(Paragraph::new(format!(" {e}")).red(), body),
                Some(Ok(view)) => view.render(f, body, self.detail.as_ref().and_then(|d| d.as_ref().ok()), &ctx.hl),
            },
        }

        if self.submit_menu {
            let mut lines = vec![Line::from(format!("{pending} pending comment(s)")).dark_gray()];
            lines.extend(SUBMIT.iter().map(|(k, label, _)| Line::from(vec![format!(" {k} ").cyan().bold(), (*label).into()])));
            let area = ui::centered(f.area(), 34, lines.len() as u16 + 2);
            f.render_widget(Clear, area);
            f.render_widget(Paragraph::new(lines).block(ui::block("submit review", true)), area);
        }
    }
}

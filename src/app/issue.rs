//! Read-only issue page: description and comments.

use super::ui;
use super::{Action, Ctx, Key, Msg};
use crate::gh::{self, model::IssueDetail, mutation::Mutation};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

pub const HELP: &[(&str, &str)] = &[
    ("j/k Ctrl-d/u", "scroll"),
    ("g/G", "top / bottom"),
    ("C", "comment (opens $EDITOR)"),
    ("o", "open in browser"),
    ("Ctrl-r", "refresh"),
    ("q / Esc", "back"),
];

pub struct IssueScreen {
    key: Key,
    detail: Option<Result<IssueDetail, String>>,
    scroll: u16,
}

impl IssueScreen {
    pub fn open(key: Key, ctx: &mut Ctx) -> Self {
        let s = Self { key, detail: None, scroll: 0 };
        s.load(ctx);
        s
    }

    fn load(&self, ctx: &mut Ctx) {
        let key = self.key;
        let (owner, name) = ctx.cfg.repos[key.0].owner_name();
        let (owner, name) = (owner.to_string(), name.to_string());
        ctx.spawn(move || Msg::Issue(key, gh::issue(&owner, &name, key.1)));
    }

    pub fn on_msg(&mut self, msg: Msg, ctx: &mut Ctx) -> Option<Msg> {
        match msg {
            Msg::Issue(key, r) if key == self.key => self.detail = Some(r.map_err(|e| e.to_string())),
            Msg::Mutated(key, ..) if key == self.key => self.load(ctx),
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
            KeyCode::Char('d') if ctrl => self.scroll += 15,
            KeyCode::Char('u') if ctrl => self.scroll = self.scroll.saturating_sub(15),
            KeyCode::Char('j') | KeyCode::Down => self.scroll += 1,
            KeyCode::Char('k') | KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Char('g') => self.scroll = 0,
            KeyCode::Char('G') => self.scroll = u16::MAX / 2,
            KeyCode::Char('o') => ctx.browse(self.key),
            KeyCode::Char('C') => {
                if let Some(Ok(d)) = &self.detail {
                    ctx.compose(self.key, &d.id, Mutation::Comment { body: String::new() });
                }
            }
            _ => {}
        }
        Action::None
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let title = format!("{} #{}", ctx.repo(self.key.0), self.key.1);
        let d = match &self.detail {
            None => return f.render_widget(Paragraph::new(" loading…").dark_gray().block(ui::block(title, true)), area),
            Some(Err(e)) => return f.render_widget(Paragraph::new(e.clone()).red().block(ui::block(title, true)), area),
            Some(Ok(d)) => d,
        };
        let state = if d.state == "OPEN" { " open ".on_green().black() } else { " closed ".on_magenta().black() };
        let mut lines = vec![
            Line::from(vec![state, " ".into(), d.title.clone().bold()]),
            Line::from(vec![
                ui::login(&d.author).into(),
                format!(" opened {} ago", ui::ago(&d.created_at)).dark_gray(),
            ]),
        ];
        let labels: Vec<Span> = d.labels.nodes.iter().map(|l| format!(" {} ", l.name).on_dark_gray()).collect();
        if !labels.is_empty() {
            lines.push(Line::from(labels));
        }
        if !d.assignees.nodes.is_empty() {
            let who: Vec<_> = d.assignees.nodes.iter().map(|a| a.login.as_str()).collect();
            lines.push(Line::from(format!("assigned: {}", who.join(", "))).dark_gray());
        }
        lines.push(Line::default());
        lines.extend(ui::comment(&d.author, &d.created_at, "".into(), &d.body, &ctx.hl));
        for c in &d.comments.nodes {
            lines.extend(ui::comment(&c.author, &c.created_at, "".into(), &c.body, &ctx.hl));
        }
        self.scroll = self.scroll.min(lines.len().saturating_sub(1) as u16);
        let p = Paragraph::new(lines).wrap(Wrap { trim: false }).scroll((self.scroll, 0)).block(ui::block(title, true));
        f.render_widget(p, area);
    }
}

//! A pull request: conversation, changed files and code buffers as tabs.

mod code;
mod conversation;
mod files;

use super::picker::{Picked, Picker};
use super::ui;
use super::{Action, Ctx, Key, Msg};
use crate::gh::{self, model::PrDetail, mutation::Mutation};
use code::Buffer;
use files::FilesView;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{Clear, Paragraph, Tabs};

pub const HELP: &[(&str, &str)] = &[
    ("1-9 H/L", "tabs: conversation, files, open buffers"),
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
    ("Ctrl-p", "open any file of the PR head"),
    ("/ n N", "buffer: search"),
    ("Ctrl-o", "jump back"),
    ("x", "close buffer"),
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

/// What a tab wants after a key press.
pub enum Outcome {
    Handled,
    Unhandled,
    /// Language-server request at (path, 0-based line, display column) of the PR head.
    Definition(String, u32, usize),
    References(String, u32, usize),
    Hover(String, u32, usize),
}

/// A place to come back to with Ctrl-o.
enum Jump {
    Conversation,
    Files((usize, usize, usize)),
    Buffer(String, usize, usize),
}

enum PickFor {
    File,
}

const FILES_TAB: usize = 1;
const FIRST_BUFFER: usize = 2;

pub struct PrScreen {
    key: Key,
    detail: Option<Result<PrDetail, String>>,
    files: Option<Result<FilesView, String>>,
    buffers: Vec<Buffer>,
    tab: usize,
    conv_scroll: u16,
    submit_menu: bool,
    picker: Option<(Picker, PickFor)>,
    /// All paths of the PR head, loaded on first Ctrl-p.
    tree: Option<Vec<String>>,
    jumps: Vec<Jump>,
}

impl PrScreen {
    pub fn open(key: Key, ctx: &mut Ctx) -> Self {
        let s = Self {
            key,
            detail: None,
            files: None,
            buffers: Vec::new(),
            tab: 0,
            conv_scroll: 0,
            submit_menu: false,
            picker: None,
            tree: None,
            jumps: Vec::new(),
        };
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

    fn files_view(&mut self) -> Option<&mut FilesView> {
        self.files.as_mut()?.as_mut().ok()
    }

    fn here(&self) -> Jump {
        match self.tab {
            0 => Jump::Conversation,
            FILES_TAB => Jump::Files(self.files.as_ref().and_then(|f| f.as_ref().ok()).map_or((0, 0, 0), |f| f.position())),
            t => {
                let b = &self.buffers[t - FIRST_BUFFER];
                Jump::Buffer(b.path.clone(), b.cursor, b.col)
            }
        }
    }

    fn jump_back(&mut self, ctx: &mut Ctx) {
        match self.jumps.pop() {
            None => ctx.info("jump list is empty"),
            Some(Jump::Conversation) => self.tab = 0,
            Some(Jump::Files(pos)) => {
                self.tab = FILES_TAB;
                if let Some(v) = self.files_view() {
                    v.set_position(pos);
                }
            }
            Some(Jump::Buffer(path, line, col)) => {
                self.open_buffer(path, line, col, ctx);
                self.jumps.pop(); // opening pushed the place we just left
            }
        }
    }

    /// Opens (or focuses) a buffer on `path` at a 0-based line and display column.
    fn open_buffer(&mut self, path: String, line: usize, col: usize, ctx: &mut Ctx) {
        self.jumps.push(self.here());
        if let Some(i) = self.buffers.iter().position(|b| b.path == path) {
            self.buffers[i].goto(line, col);
            self.tab = FIRST_BUFFER + i;
            return;
        }
        let Some(oid) = self.detail().map(|d| d.head_ref_oid.clone()) else { return };
        self.buffers.push(Buffer::loading(path.clone(), line, col));
        self.tab = FIRST_BUFFER + self.buffers.len() - 1;
        let (key, src) = (self.key, ctx.sources[self.key.0].clone());
        ctx.spawn(move || {
            let text = if path.starts_with('/') { std::fs::read_to_string(&path).map_err(Into::into) } else { src.read(&oid, &path) };
            Msg::FileText(key, path, text)
        });
    }

    fn open_finder(&mut self, ctx: &mut Ctx) {
        match &self.tree {
            Some(paths) => self.picker = Some((Picker::new("files", paths.clone()), PickFor::File)),
            None => {
                let Some(oid) = self.detail().map(|d| d.head_ref_oid.clone()) else { return };
                let (key, src) = (self.key, ctx.sources[self.key.0].clone());
                ctx.spawn(move || Msg::Tree(key, src.list(&oid)));
                ctx.info("loading file list…");
            }
        }
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
            Msg::Tree(key, r) if key == self.key => match r {
                Ok(paths) => {
                    self.tree = Some(paths);
                    self.open_finder(ctx);
                }
                Err(e) => ctx.error(e),
            },
            Msg::FileText(key, path, r) if key == self.key => {
                if let Some(b) = self.buffers.iter_mut().find(|b| b.path == path) {
                    b.loaded(r.map_err(|e| e.to_string()), &ctx.hl);
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
        if let Some((picker, _)) = &mut self.picker {
            match picker.on_key(k) {
                Picked::Pending => {}
                Picked::Cancel => self.picker = None,
                Picked::Pick(i) => {
                    let (_, for_) = self.picker.take().unwrap();
                    match for_ {
                        PickFor::File => {
                            let path = self.tree.as_ref().unwrap()[i].clone();
                            self.open_buffer(path, 0, 0, ctx);
                        }
                    }
                }
            }
            return Action::None;
        }

        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Char('p') if ctrl => {
                self.open_finder(ctx);
                return Action::None;
            }
            KeyCode::Char('o') if ctrl => {
                self.jump_back(ctx);
                return Action::None;
            }
            _ => {}
        }

        let outcome = match self.tab {
            0 => Outcome::Unhandled,
            FILES_TAB => {
                let detail = self.detail.as_ref().and_then(|d| d.as_ref().ok());
                match self.files.as_mut().and_then(|f| f.as_mut().ok()) {
                    Some(view) => view.on_key(k, ctx, self.key, detail),
                    None => Outcome::Unhandled,
                }
            }
            t => self.buffers[t - FIRST_BUFFER].on_key(k),
        };
        match outcome {
            Outcome::Handled => return Action::None,
            Outcome::Unhandled => {}
            // Wired to the language server in a later step.
            Outcome::Definition(..) | Outcome::References(..) | Outcome::Hover(..) => {
                ctx.info("no language server yet");
                return Action::None;
            }
        }

        let tabs = FIRST_BUFFER + self.buffers.len();
        match k.code {
            KeyCode::Char('q') | KeyCode::Esc => return Action::Pop,
            KeyCode::Char('?') => return Action::Help,
            KeyCode::Char('r') if ctrl => {
                self.load(ctx);
                self.load_files(ctx);
            }
            KeyCode::Char('o') => ctx.browse(self.key),
            KeyCode::Char(c @ '1'..='9') => {
                let t = c as usize - '1' as usize;
                if t < tabs {
                    self.tab = t;
                }
            }
            KeyCode::Char('L') => self.tab = (self.tab + 1) % tabs,
            KeyCode::Char('H') => self.tab = (self.tab + tabs - 1) % tabs,
            KeyCode::Char('x') if self.tab >= FIRST_BUFFER => {
                self.buffers.remove(self.tab - FIRST_BUFFER);
                self.tab -= 1;
            }
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
        let mut titles = vec![Line::from(" 1 conversation "), Line::from(files_title)];
        for (i, b) in self.buffers.iter().enumerate() {
            let name = b.path.rsplit('/').next().unwrap_or(&b.path);
            titles.push(Line::from(format!(" {} {name} ", i + FIRST_BUFFER + 1)));
        }
        let tabs = Tabs::new(titles).select(self.tab).divider("").highlight_style(Style::new().fg(ui::ACCENT).bold().reversed());
        f.render_widget(tabs, tabs_area);

        match self.tab {
            0 => self.conv_scroll = conversation::render(f, body, d, &ctx.hl, self.conv_scroll),
            FILES_TAB => match &mut self.files {
                None => f.render_widget(Paragraph::new(" loading files…").dark_gray(), body),
                Some(Err(e)) => f.render_widget(Paragraph::new(format!(" {e}")).red(), body),
                Some(Ok(view)) => view.render(f, body, self.detail.as_ref().and_then(|d| d.as_ref().ok()), &ctx.hl),
            },
            t => self.buffers[t - FIRST_BUFFER].render(f, body),
        }

        if self.submit_menu {
            let mut lines = vec![Line::from(format!("{pending} pending comment(s)")).dark_gray()];
            lines.extend(SUBMIT.iter().map(|(k, label, _)| Line::from(vec![format!(" {k} ").cyan().bold(), (*label).into()])));
            let area = ui::centered(f.area(), 34, lines.len() as u16 + 2);
            f.render_widget(Clear, area);
            f.render_widget(Paragraph::new(lines).block(ui::block("submit review", true)), area);
        }
        if let Some((picker, _)) = &mut self.picker {
            picker.render(f);
        }
    }
}

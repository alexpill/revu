//! A pull request: conversation, changed files and code buffers as tabs.

mod code;
mod conversation;
mod files;

use super::picker::{Picked, Picker};
use super::ui;
use super::{Action, Ctx, Key, Msg};
use crate::gh::{self, model::PrDetail, mutation::Mutation};
use crate::lsp;
use anyhow::{Result, anyhow};
use code::Buffer;
use files::FilesView;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{Clear, Paragraph, Tabs, Wrap};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

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
    ("D", "on a comment: delete it (pending or yours)"),
    ("z", "unfold generated file"),
    ("gd / gr / K", "definition / references / hover (language server)"),
    ("e", "open here in $EDITOR"),
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
    Location(Vec<lsp::Location>),
}

#[derive(Clone, Copy)]
enum AskKind {
    Definition,
    References,
    Hover,
}

/// A language-server question about a position of the PR head.
#[derive(Clone)]
struct Ask {
    kind: AskKind,
    path: String,
    line: u32,
    col: usize,
}

/// How long after start an empty answer is blamed on loading (and retried).
const WARMUP: Duration = Duration::from_secs(300);

fn is_empty_answer(kind: AskKind, v: &serde_json::Value) -> bool {
    match kind {
        AskKind::Hover => lsp::hover_text(v).is_none(),
        _ => lsp::locations(v).is_empty(),
    }
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
    /// Detached checkout of the PR head that language servers run in.
    worktree: Option<Result<PathBuf, String>>,
    /// Waiting for the worktree.
    pending_ask: Option<Ask>,
    servers: HashMap<Vec<String>, lsp::Server>,
    lsp_requests: HashMap<i64, Ask>,
    /// Servers (by id) that haven't finished their first load, with start time.
    warming: HashMap<i64, Instant>,
    /// Asked while the server was still loading: re-sent once it is ready.
    retry: Option<Ask>,
    hover: Option<String>,
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
            worktree: None,
            pending_ask: None,
            servers: HashMap::new(),
            lsp_requests: HashMap::new(),
            warming: HashMap::new(),
            retry: None,
            hover: None,
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
            FILES_TAB => Jump::Files(
                self.files
                    .as_ref()
                    .and_then(|f| f.as_ref().ok())
                    .map_or((0, 0, 0), |f| f.position()),
            ),
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
        let Some(oid) = self.detail().map(|d| d.head_ref_oid.clone()) else {
            return;
        };
        self.buffers.push(Buffer::loading(path.clone(), line, col));
        self.tab = FIRST_BUFFER + self.buffers.len() - 1;
        let (key, src) = (self.key, ctx.sources[self.key.0].clone());
        ctx.spawn(move || {
            let text = if path.starts_with('/') {
                std::fs::read_to_string(&path).map_err(Into::into)
            } else {
                src.read(&oid, &path)
            };
            Msg::FileText(key, path, text)
        });
    }

    fn ask(&mut self, ask: Ask, ctx: &mut Ctx) {
        match &self.worktree {
            Some(Ok(root)) => {
                let root = root.clone();
                if let Err(e) = self.send_ask(&root, ask, ctx) {
                    ctx.error(e);
                }
            }
            Some(Err(e)) => ctx.error(e.clone()),
            None => {
                let first = self.pending_ask.replace(ask).is_none();
                let Some(oid) = self.detail().map(|d| d.head_ref_oid.clone()) else {
                    return;
                };
                if first {
                    let (key, src) = (self.key, ctx.sources[self.key.0].clone());
                    ctx.spawn(move || Msg::Worktree(key, src.worktree(key.1, &oid)));
                    ctx.info("preparing a worktree of the PR head for the language server…");
                }
            }
        }
    }

    fn ask_none(&mut self, kind: AskKind, path: String, line: u32, col: usize, ctx: &mut Ctx) -> Action {
        self.ask(Ask { kind, path, line, col }, ctx);
        Action::None
    }

    fn send_ask(&mut self, root: &Path, ask: Ask, ctx: &mut Ctx) -> Result<()> {
        let abs = if ask.path.starts_with('/') {
            PathBuf::from(&ask.path)
        } else {
            root.join(&ask.path)
        };
        let (cmd, lang) = lsp::server_for(&ask.path, &ctx.cfg.lsp)
            .ok_or_else(|| anyhow!("no language server known for {} (add one under [lsp])", ask.path))?;
        if !self.servers.contains_key(&cmd) {
            let tx = ctx.sender();
            let server = lsp::Server::start(&cmd, root, move |id, event| {
                let _ = tx.send(Msg::Lsp(id, event));
            })?;
            self.warming.insert(server.id, Instant::now());
            self.servers.insert(cmd.clone(), server);
        }
        let server = self.servers.get_mut(&cmd).unwrap();
        let text = std::fs::read_to_string(&abs)?;
        server.open(&abs, &lang, &text);
        let character = lsp::utf16_col(text.lines().nth(ask.line as usize).unwrap_or(""), ask.col);
        let (line, kind) = (ask.line, ask.kind);
        let method = match kind {
            AskKind::Definition => "textDocument/definition",
            AskKind::References => "textDocument/references",
            AskKind::Hover => "textDocument/hover",
        };
        let id = server.request(method, &abs, line, character);
        self.lsp_requests.insert(id, ask);
        ctx.info(format!("asking {}…", cmd[0]));
        Ok(())
    }

    fn on_lsp_event(&mut self, server: i64, event: lsp::Event, ctx: &mut Ctx) {
        let name = self
            .servers
            .iter()
            .find(|(_, s)| s.id == server)
            .map_or(String::new(), |(c, _)| c[0].clone());
        match event {
            lsp::Event::Response(id, r) => {
                if let Some(ask) = self.lsp_requests.remove(&id) {
                    // Servers answer "nothing" while loading instead of waiting.
                    let warming = self.warming.get(&server).is_some_and(|t| t.elapsed() < WARMUP);
                    if warming && r.as_ref().is_ok_and(|v| is_empty_answer(ask.kind, v)) {
                        self.retry = Some(ask);
                        return ctx.info(format!("{name} is loading the project, will answer when ready…"));
                    }
                    self.on_lsp(ask.kind, r, ctx);
                }
            }
            lsp::Event::Status(text) => ctx.info(format!("{name}: {text}")),
            lsp::Event::Idle => {
                if self.warming.remove(&server).is_some() {
                    match self.retry.take() {
                        Some(ask) => self.ask(ask, ctx),
                        None => ctx.info(format!("{name} ready")),
                    }
                }
            }
            lsp::Event::Exited(stderr) => {
                // Dropped so the next request starts a fresh one.
                self.servers.retain(|_, s| s.id != server);
                self.lsp_requests.clear();
                self.warming.remove(&server);
                self.retry = None;
                let why = if stderr.is_empty() {
                    "no error output".into()
                } else {
                    stderr
                };
                ctx.error(format!("{name} exited: {why}"));
            }
        }
    }

    fn on_lsp(&mut self, kind: AskKind, r: Result<serde_json::Value, String>, ctx: &mut Ctx) {
        let empty = |ctx: &mut Ctx| ctx.info("nothing found");
        let v = match r {
            Ok(v) => v,
            Err(e) => return ctx.error(format!("language server: {e}")),
        };
        ctx.clear_status();
        match kind {
            AskKind::Hover => match lsp::hover_text(&v) {
                Some(t) => self.hover = Some(t),
                None => empty(ctx),
            },
            AskKind::Definition | AskKind::References => {
                let locs = lsp::locations(&v);
                match (kind, locs.len()) {
                    (_, 0) => empty(ctx),
                    (AskKind::Definition, 1) => self.open_location(&locs[0], ctx),
                    _ => self.pick_locations(locs),
                }
            }
        }
    }

    /// Repo-relative when inside the worktree, absolute otherwise.
    fn display_path(&self, abs: &Path) -> String {
        let root = self.worktree.as_ref().and_then(|w| w.as_ref().ok());
        match root.and_then(|r| abs.strip_prefix(r).ok()) {
            Some(rel) => rel.to_string_lossy().into_owned(),
            None => abs.to_string_lossy().into_owned(),
        }
    }

    fn open_location(&mut self, (abs, line, character): &lsp::Location, ctx: &mut Ctx) {
        let col = lsp::read_line(abs, *line).map_or(0, |l| lsp::display_col(&l, *character));
        self.open_buffer(self.display_path(abs), *line as usize, col, ctx);
    }

    fn pick_locations(&mut self, locs: Vec<lsp::Location>) {
        let items = locs
            .iter()
            .map(|(abs, line, _)| {
                let text = lsp::read_line(abs, *line).unwrap_or_default();
                format!("{}:{}  {}", self.display_path(abs), line + 1, text.trim())
            })
            .collect();
        self.picker = Some((Picker::new("locations", items), PickFor::Location(locs)));
    }

    /// Opens the current position in $EDITOR, in the worktree when there is one.
    fn edit_here(&mut self, ctx: &mut Ctx) {
        let here = match self.tab {
            FILES_TAB => self
                .files
                .as_ref()
                .and_then(|f| f.as_ref().ok())
                .and_then(|f| f.head_position())
                .map(|(p, l, _)| (p, l)),
            t if t >= FIRST_BUFFER => self
                .buffers
                .get(t - FIRST_BUFFER)
                .map(|b| (b.path.clone(), b.cursor as u32)),
            _ => None,
        };
        let Some((path, line)) = here else {
            return ctx.info("nothing to open here");
        };
        let base = match (&self.worktree, &ctx.sources[self.key.0].local) {
            _ if path.starts_with('/') => PathBuf::new(),
            (Some(Ok(wt)), _) => wt.clone(),
            (_, Some(local)) => local.clone(), // may not be at the PR head
            _ => return ctx.error("needs a local clone: set `path` for this repo in the config"),
        };
        ctx.open_in_editor(base.join(path), line + 1);
    }

    fn open_finder(&mut self, ctx: &mut Ctx) {
        match &self.tree {
            Some(paths) => self.picker = Some((Picker::new("files", paths.clone()), PickFor::File)),
            None => {
                let Some(oid) = self.detail().map(|d| d.head_ref_oid.clone()) else {
                    return;
                };
                let (key, src) = (self.key, ctx.sources[self.key.0].clone());
                ctx.spawn(move || Msg::Tree(key, src.list(&oid)));
                ctx.info("loading file list…");
            }
        }
    }

    pub fn on_msg(&mut self, msg: Msg, ctx: &mut Ctx) -> Option<Msg> {
        match msg {
            Msg::Pr(key, r) if key == self.key => {
                // shortcut: a reload already in flight when S was pressed clears this early; fine, the next one is right.
                ctx.submitting.retain(|k| *k != key);
                self.detail = Some(r.map_err(|e| e.to_string()));
            }
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
            Msg::Worktree(key, r) if key == self.key => {
                self.worktree = Some(r.map_err(|e| e.to_string()));
                if let Some(ask) = self.pending_ask.take() {
                    self.ask(ask, ctx);
                }
            }
            Msg::Lsp(server, event) if self.servers.values().any(|s| s.id == server) => {
                self.on_lsp_event(server, event, ctx)
            }
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
        if self.hover.take().is_some() {
            return Action::None;
        }
        if self.submit_menu {
            self.submit_menu = false;
            if let (KeyCode::Char(c), Some(d)) = (k.code, self.detail()) {
                let id = d.id.clone();
                match SUBMIT.iter().find(|(key, ..)| *key == c) {
                    Some((_, _, Some(event))) => ctx.compose(
                        self.key,
                        &id,
                        Mutation::Submit {
                            event,
                            body: String::new(),
                        },
                    ),
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
                        PickFor::Location(locs) => self.open_location(&locs[i], ctx),
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
            Outcome::Definition(path, line, col) => return self.ask_none(AskKind::Definition, path, line, col, ctx),
            Outcome::References(path, line, col) => return self.ask_none(AskKind::References, path, line, col, ctx),
            Outcome::Hover(path, line, col) => return self.ask_none(AskKind::Hover, path, line, col, ctx),
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
            KeyCode::Char('e') => self.edit_here(ctx),
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
            None => {
                return f.render_widget(
                    Paragraph::new(format!(" {repo} #{} loading…", self.key.1)).dark_gray(),
                    head,
                );
            }
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
        if ctx.submitting.contains(&self.key) {
            title.push("  submitting review…".yellow());
        } else if pending > 0 {
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
        let tabs = Tabs::new(titles)
            .select(self.tab)
            .divider("")
            .highlight_style(Style::new().fg(ui::ACCENT).bold().reversed());
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
            lines.extend(
                SUBMIT
                    .iter()
                    .map(|(k, label, _)| Line::from(vec![format!(" {k} ").cyan().bold(), (*label).into()])),
            );
            let area = ui::centered(f.area(), 34, lines.len() as u16 + 2);
            f.render_widget(Clear, area);
            f.render_widget(Paragraph::new(lines).block(ui::block("submit review", true)), area);
        }
        if let Some((picker, _)) = &mut self.picker {
            picker.render(f);
        }
        if let Some(text) = &self.hover {
            let lines = crate::markdown::render(text, &ctx.hl);
            let area = ui::centered(f.area(), 90, lines.len() as u16 + 2);
            f.render_widget(Clear, area);
            f.render_widget(
                Paragraph::new(lines)
                    .wrap(Wrap { trim: false })
                    .block(ui::block("hover", true)),
                area,
            );
        }
    }
}

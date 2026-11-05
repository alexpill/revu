//! Application shell: event loop, screen stack, background jobs.
//!
//! Background work (gh calls, git, LSP I/O) runs on plain threads that report
//! back through a single `Msg` channel; the UI thread never blocks on I/O.

mod issue;
mod main_screen;
mod picker;
mod pr;
pub mod ui;

use crate::config::{self, Config};
use crate::gh::model::*;
use crate::gh::mutation::{self, Job, Mutation};
use crate::highlight::Highlighter;
use crate::source::Source;
use anyhow::Result;
use ratatui::DefaultTerminal;
use ratatui::Frame;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton,
    MouseEventKind,
};
use ratatui::crossterm::execute;
use ratatui::layout::{Constraint, Layout, Position};
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use std::path::PathBuf;
use std::process::Command;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Duration;

/// (index of the repo in the config, PR or issue number)
pub type Key = (usize, u64);

pub enum Msg {
    RepoPrs(usize, Result<RepoPrs>),
    Issues(usize, Result<Vec<IssueSummary>>),
    Issue(Key, Result<IssueDetail>),
    Pr(Key, Result<PrDetail>),
    Files(Key, Result<(Vec<ChangedFile>, std::collections::HashMap<String, bool>)>),
    Fetched(Key, Result<()>),
    /// All paths at the PR head.
    Tree(Key, Result<Vec<String>>),
    FileText(Key, String, Result<String>),
    Worktree(Key, Result<PathBuf>),
    /// From the language server with this id (not counted as a background job).
    Lsp(i64, crate::lsp::Event),
    Mutated(Key, Mutation, Result<()>),
}

/// Something that needs the terminal, run between frames.
pub enum External {
    /// Write a comment in $EDITOR, then queue `mutation` with it as body.
    Compose {
        key: Key,
        pr_id: String,
        mutation: Mutation,
    },
    /// Open a file at a 1-based line in $EDITOR.
    Open { path: PathBuf, line: u32 },
}

pub enum Action {
    None,
    Push(Screen),
    Pop,
    Quit,
    Help,
}

pub enum Screen {
    Issue(issue::IssueScreen),
    Pr(Box<pr::PrScreen>),
}

impl Screen {
    fn on_key(&mut self, k: KeyEvent, ctx: &mut Ctx) -> Action {
        match self {
            Screen::Issue(s) => s.on_key(k, ctx),
            Screen::Pr(s) => s.on_key(k, ctx),
        }
    }
    fn on_click(&mut self, pos: Position, ctx: &mut Ctx) -> Action {
        match self {
            Screen::Issue(_) => Action::None,
            Screen::Pr(s) => s.on_click(pos, ctx),
        }
    }
    fn on_msg(&mut self, msg: Msg, ctx: &mut Ctx) -> Option<Msg> {
        match self {
            Screen::Issue(s) => s.on_msg(msg, ctx),
            Screen::Pr(s) => s.on_msg(msg, ctx),
        }
    }
    fn render(&mut self, f: &mut Frame, area: ratatui::layout::Rect, ctx: &Ctx) {
        match self {
            Screen::Issue(s) => s.render(f, area, ctx),
            Screen::Pr(s) => s.render(f, area, ctx),
        }
    }
    fn help(&self) -> &'static [(&'static str, &'static str)] {
        match self {
            Screen::Issue(_) => issue::HELP,
            Screen::Pr(_) => pr::HELP,
        }
    }
}

/// State shared with every screen.
pub struct Ctx {
    pub cfg: Config,
    pub sources: Vec<Source>,
    pub hl: Highlighter,
    pub viewer: String,
    tx: Sender<Msg>,
    worker: Sender<Job<Key>>,
    /// Background jobs not yet reported back, for the activity indicator.
    inflight: usize,
    status: Option<(String, bool)>,
    external: Option<External>,
    /// PRs whose review submit/discard is sent but not yet reloaded.
    pub submitting: Vec<Key>,
}

impl Ctx {
    pub fn spawn(&mut self, job: impl FnOnce() -> Msg + Send + 'static) {
        self.inflight += 1;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(job());
        });
    }

    pub fn mutate(&mut self, key: Key, pr_id: &str, mutation: Mutation) {
        self.inflight += 1;
        if matches!(mutation, Mutation::Submit { .. } | Mutation::DiscardReview) {
            self.submitting.push(key);
        }
        let _ = self.worker.send(Job {
            key,
            pr_id: pr_id.to_string(),
            mutation,
        });
    }

    pub fn compose(&mut self, key: Key, pr_id: &str, mutation: Mutation) {
        self.external = Some(External::Compose {
            key,
            pr_id: pr_id.to_string(),
            mutation,
        });
    }

    pub fn open_in_editor(&mut self, path: PathBuf, line: u32) {
        self.external = Some(External::Open { path, line });
    }

    pub fn sender(&self) -> Sender<Msg> {
        self.tx.clone()
    }

    pub fn clear_status(&mut self) {
        self.status = None;
    }

    pub fn info(&mut self, s: impl Into<String>) {
        self.status = Some((s.into(), false));
    }

    pub fn error(&mut self, e: impl std::fmt::Display) {
        self.status = Some((e.to_string(), true));
    }

    pub fn repo(&self, i: usize) -> &str {
        &self.cfg.repos[i].name
    }

    pub fn browse(&mut self, key: Key) {
        let repo = self.repo(key.0).to_string();
        if let Err(e) = crate::gh::browse(&repo, key.1) {
            self.error(e);
        }
    }
}

pub struct App {
    ctx: Ctx,
    rx: Receiver<Msg>,
    main: main_screen::MainScreen,
    stack: Vec<Screen>,
    help: bool,
    quit: bool,
}

impl App {
    pub fn new(cfg: Config) -> Self {
        let (tx, rx) = channel();
        let done_tx = tx.clone();
        let worker = mutation::spawn_worker(move |job: Job<Key>, res| {
            let _ = done_tx.send(Msg::Mutated(job.key, job.mutation, res));
        });
        let sources = cfg
            .repos
            .iter()
            .map(|r| Source {
                repo: r.name.clone(),
                local: r.path.clone(),
            })
            .collect();
        let hl = Highlighter::new(cfg.theme.as_deref());
        let mut ctx = Ctx {
            cfg,
            sources,
            hl,
            viewer: String::new(),
            tx,
            worker,
            inflight: 0,
            status: None,
            external: None,
            submitting: Vec::new(),
        };
        let main = main_screen::MainScreen::new(&mut ctx);
        Self {
            ctx,
            rx,
            main,
            stack: Vec::new(),
            help: false,
            quit: false,
        }
    }

    pub fn run(mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        let mut dirty = true;
        while !self.quit {
            if dirty {
                terminal.draw(|f| self.render(f))?;
                dirty = false;
            }
            // Handle every queued key before redrawing, so held keys don't lag.
            let mut timeout = Duration::from_millis(50);
            while event::poll(timeout)? {
                match event::read()? {
                    Event::Key(k) if k.kind == KeyEventKind::Press => self.on_key(k),
                    // The wheel scrolls whatever has focus, like j/k.
                    Event::Mouse(m) if m.kind == MouseEventKind::ScrollDown => self.on_key(KeyCode::Down.into()),
                    Event::Mouse(m) if m.kind == MouseEventKind::ScrollUp => self.on_key(KeyCode::Up.into()),
                    Event::Mouse(m) if m.kind == MouseEventKind::Down(MouseButton::Left) => {
                        self.on_click(Position::new(m.column, m.row))
                    }
                    // Motion reports flood in while the pointer moves; don't redraw for them.
                    Event::Mouse(_) => {
                        timeout = Duration::ZERO;
                        continue;
                    }
                    _ => {}
                }
                dirty = true;
                timeout = Duration::ZERO;
                if self.ctx.external.is_some() || self.quit {
                    break;
                }
            }
            while let Ok(msg) = self.rx.try_recv() {
                self.on_msg(msg);
                dirty = true;
            }
            if let Some(ext) = self.ctx.external.take() {
                self.run_external(ext, terminal);
                terminal.clear()?;
                dirty = true;
            }
        }
        Ok(())
    }

    fn on_key(&mut self, k: KeyEvent) {
        if k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL) {
            self.quit = true;
            return;
        }
        if self.help {
            self.help = false;
            return;
        }
        self.ctx.status = None;
        let action = match self.stack.last_mut() {
            Some(s) => s.on_key(k, &mut self.ctx),
            None => self.main.on_key(k, &mut self.ctx),
        };
        self.apply(action);
    }

    fn on_click(&mut self, pos: Position) {
        if self.help {
            self.help = false;
            return;
        }
        self.ctx.status = None;
        let action = match self.stack.last_mut() {
            Some(s) => s.on_click(pos, &mut self.ctx),
            None => self.main.on_click(pos, &mut self.ctx),
        };
        self.apply(action);
    }

    fn apply(&mut self, action: Action) {
        match action {
            Action::None => {}
            Action::Push(s) => self.stack.push(s),
            Action::Pop => {
                if self.stack.pop().is_none() {
                    self.quit = true;
                }
            }
            Action::Quit => self.quit = true,
            Action::Help => self.help = true,
        }
    }

    fn on_msg(&mut self, msg: Msg) {
        if !matches!(msg, Msg::Lsp(..)) {
            self.ctx.inflight = self.ctx.inflight.saturating_sub(1);
        }
        let msg = match msg {
            Msg::RepoPrs(i, r) => return self.main.on_prs(i, r, &mut self.ctx),
            Msg::Issues(i, r) => return self.main.on_issues(i, r, &mut self.ctx),
            Msg::Mutated(key, m, res) => {
                match &res {
                    Ok(()) if m.needs_reload() => self.ctx.info("saved"),
                    Ok(()) => {}
                    Err(e) => self.ctx.error(format!("GitHub sync failed: {e}")),
                }
                Msg::Mutated(key, m, res)
            }
            m => m,
        };
        let mut msg = Some(msg);
        for s in self.stack.iter_mut().rev() {
            msg = s.on_msg(msg.take().unwrap(), &mut self.ctx);
            if msg.is_none() {
                break;
            }
        }
    }

    fn run_external(&mut self, ext: External, _terminal: &mut DefaultTerminal) {
        match ext {
            External::Compose { key, pr_id, mutation } => {
                let file = config::cache_dir().join("COMMENT.md");
                let allow_empty = matches!(mutation, Mutation::Submit { .. });
                match compose(&file) {
                    Ok(body) if body.is_empty() && !allow_empty => self.ctx.info("empty comment, cancelled"),
                    Ok(body) => {
                        self.ctx.mutate(key, &pr_id, mutation.with_body(body));
                        self.ctx.info("sending…");
                    }
                    Err(e) => self.ctx.info(format!("cancelled ({e})")),
                }
            }
            External::Open { path, line } => {
                if let Err(e) = with_terminal_released(&mut editor(&[&format!("+{line}"), &path.to_string_lossy()])) {
                    self.ctx.error(e);
                }
            }
        }
    }

    fn render(&mut self, f: &mut Frame) {
        let [body, bar] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(f.area());
        let help = match self.stack.last_mut() {
            Some(s) => {
                s.render(f, body, &self.ctx);
                s.help()
            }
            None => {
                self.main.render(f, body, &self.ctx);
                main_screen::HELP
            }
        };

        let mut right = Vec::new();
        if self.ctx.inflight > 0 {
            right.push("⟳ ".yellow());
        }
        if let Some((s, err)) = &self.ctx.status {
            right.push(if *err { s.clone().red() } else { s.clone().green() });
        }
        let right = Line::from(right).right_aligned();
        f.render_widget(Paragraph::new(Line::from(" ? help").dark_gray()), bar);
        f.render_widget(right, bar);

        if self.help {
            let lines: Vec<Line> = help
                .iter()
                .map(|(k, d)| Line::from(vec![Span::from(format!("{k:>16}  ")).cyan(), Span::from(*d)]))
                .collect();
            let area = ui::centered(f.area(), 76, lines.len() as u16 + 2);
            f.render_widget(Clear, area);
            f.render_widget(Paragraph::new(lines).block(ui::block("keys", true)), area);
        }
    }
}

/// `ratatui::init` plus mouse reporting.
pub fn init_terminal() -> DefaultTerminal {
    let terminal = ratatui::init();
    let _ = execute!(std::io::stdout(), EnableMouseCapture);
    terminal
}

pub fn restore_terminal() {
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
}

/// Runs a command with the terminal handed over to it.
pub fn with_terminal_released(cmd: &mut Command) -> Result<()> {
    restore_terminal();
    let status = cmd.status();
    init_terminal();
    let status = status?;
    anyhow::ensure!(status.success(), "{cmd:?} exited with {status}");
    Ok(())
}

/// `$EDITOR` (shell-split, e.g. "nvim -u NONE") with extra args.
pub fn editor(args: &[&str]) -> Command {
    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg("exec ${VISUAL:-${EDITOR:-vi}} \"$@\"")
        .arg("sh")
        .args(args);
    cmd
}

/// Lets the user write a comment; errors when the editor exits non-zero (`:cq`).
fn compose(file: &PathBuf) -> Result<String> {
    std::fs::create_dir_all(file.parent().unwrap())?;
    std::fs::write(file, "")?;
    with_terminal_released(&mut editor(&[&file.to_string_lossy()]))?;
    Ok(std::fs::read_to_string(file)?.trim().to_string())
}

//! "Files changed": file list + diff with inline review threads.

use super::{Outcome, conversation};
use crate::app::ui::{self, DIM, SELECTED};
use crate::app::{Ctx, Key};
use crate::diff::{self, DiffLine, Kind};
use crate::gh::model::{ChangedFile, PrDetail, Thread};
use crate::gh::mutation::Mutation;
use crate::highlight::{Highlighter, StyledLine};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent};
use ratatui::layout::{Margin, Position, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};
use std::collections::HashMap;

const ADD_BG: Color = Color::Rgb(22, 50, 32);
const DEL_BG: Color = Color::Rgb(62, 26, 30);
const THREAD_BG: Color = Color::Rgb(28, 30, 40);
/// "1234 1234 + " before the code.
const GUTTER: usize = 12;

const GENERATED: &[&str] = &[
    "Cargo.lock",
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "flake.lock",
    "poetry.lock",
    "uv.lock",
    "go.sum",
    "Gemfile.lock",
    "composer.lock",
    "bun.lockb",
];

pub struct FileEntry {
    pub path: String,
    prev: Option<String>,
    status: String,
    additions: u64,
    deletions: u64,
    pub lines: Vec<DiffLine>,
    has_patch: bool,
    pub viewed: bool,
    /// Lockfiles & co. start folded.
    folded: bool,
    /// Per diff line, computed on first display.
    hl: Option<Vec<StyledLine>>,
}

pub fn entries(files: Vec<ChangedFile>, viewed: &HashMap<String, bool>) -> Vec<FileEntry> {
    let mut out: Vec<FileEntry> = files
        .into_iter()
        .map(|f| {
            let name = f.filename.rsplit('/').next().unwrap_or(&f.filename);
            let generated = GENERATED.contains(&name) || name.ends_with(".snap") || name.ends_with(".min.js");
            FileEntry {
                viewed: viewed.get(&f.filename).copied().unwrap_or(false),
                lines: f.patch.as_deref().map(diff::parse).unwrap_or_default(),
                has_patch: f.patch.is_some(),
                path: f.filename,
                prev: f.previous_filename,
                status: f.status,
                additions: f.additions,
                deletions: f.deletions,
                folded: generated,
                hl: None,
            }
        })
        .collect();
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

pub enum Row {
    /// Diff line `.0`, segment `.1` of it when wrapped (0 otherwise).
    Line(usize, usize),
    /// A rendered line of review thread `.0` (index in `review_threads`),
    /// showing its comment `.1` (`None` on the header).
    Thread(usize, Option<usize>, Line<'static>),
    Note(Line<'static>),
}

pub struct FilesView {
    pub files: Vec<FileEntry>,
    pub sel: usize,
    focus_diff: bool,
    cursor: usize,
    pub col: usize,
    scroll: usize,
    /// First code column shown when lines aren't wrapped.
    hscroll: usize,
    wrap: bool,
    hide_list: bool,
    split: ui::Split,
    /// Columns left for code after the gutter, as last rendered.
    code_w: usize,
    /// Start row of a visual (range) selection.
    anchor: Option<usize>,
    pending_g: bool,
    /// Comment awaiting `y` to be deleted.
    confirm_delete: Option<String>,
    /// Rows as last rendered, so keys act on what is on screen.
    rows: Vec<Row>,
    height: usize,
    list: ListState,
    list_area: Rect,
    diff_area: Rect,
}

impl FilesView {
    pub fn new(files: Vec<FileEntry>) -> Self {
        Self {
            files,
            sel: 0,
            focus_diff: false,
            cursor: 0,
            col: 0,
            scroll: 0,
            hscroll: 0,
            wrap: false,
            hide_list: false,
            split: ui::Split::default(),
            code_w: 80,
            anchor: None,
            pending_g: false,
            confirm_delete: None,
            rows: Vec::new(),
            height: 20,
            list: ListState::default(),
            list_area: Rect::default(),
            diff_area: Rect::default(),
        }
    }

    /// Keeps the selection on the same file after a reload.
    pub fn replace(&mut self, files: Vec<FileEntry>) {
        let path = self.files.get(self.sel).map(|f| f.path.clone());
        self.files = files;
        if let Some(i) = path.and_then(|p| self.files.iter().position(|f| f.path == p)) {
            self.sel = i;
        } else {
            self.select(0);
        }
    }

    pub fn viewed_count(&self) -> usize {
        self.files.iter().filter(|f| f.viewed).count()
    }

    /// (file, row, column), for the jump list.
    pub fn position(&self) -> (usize, usize, usize) {
        (self.sel, self.cursor, self.col)
    }

    pub fn set_position(&mut self, (sel, cursor, col): (usize, usize, usize)) {
        self.select(sel);
        self.cursor = cursor;
        self.col = col;
        self.focus_diff = true;
    }

    fn select(&mut self, i: usize) {
        self.sel = i.min(self.files.len().saturating_sub(1));
        self.cursor = 0;
        self.scroll = 0;
        self.hscroll = 0;
        self.col = 0;
        self.anchor = None;
        self.rows.clear(); // they are the previous file's
    }

    /// Width of a wrapped segment: a line is one segment when not wrapping.
    fn seg_w(&self) -> usize {
        if self.wrap { self.code_w } else { usize::MAX }
    }

    fn current_line(&self) -> Option<&DiffLine> {
        match self.rows.get(self.cursor)? {
            Row::Line(i, _) => self.files.get(self.sel)?.lines.get(*i),
            _ => None,
        }
    }

    fn current_thread(&self) -> Option<usize> {
        match self.rows.get(self.cursor)? {
            Row::Thread(t, ..) => Some(*t),
            _ => None,
        }
    }

    fn line_chars(&self) -> Vec<char> {
        self.current_line()
            .map(|l| l.text.replace('\t', "    ").chars().collect())
            .unwrap_or_default()
    }

    fn move_cursor(&mut self, delta: isize) {
        let max = self.rows.len().saturating_sub(1);
        self.cursor = self.cursor.saturating_add_signed(delta).min(max);
    }

    fn jump_hunk(&mut self, forward: bool) {
        let Some(f) = self.files.get(self.sel) else { return };
        let is_hunk = |r: &Row| matches!(r, Row::Line(i, 0) if f.lines[*i].kind == Kind::Hunk);
        let found = if forward {
            (self.cursor + 1..self.rows.len()).find(|&r| is_hunk(&self.rows[r]))
        } else {
            (0..self.cursor).rev().find(|&r| is_hunk(&self.rows[r]))
        };
        if let Some(r) = found {
            self.cursor = r;
            self.scroll = r; // hunk header at the top of the view
        }
    }

    /// (path, 0-based line, display column) on the PR head; `None` on removed lines.
    pub fn head_position(&self) -> Option<(String, u32, usize)> {
        let l = self.current_line()?;
        let n = l.new.filter(|_| l.kind != Kind::Del)?;
        Some((self.files[self.sel].path.clone(), n - 1, self.col))
    }

    pub fn on_key(&mut self, k: KeyEvent, ctx: &mut Ctx, key: Key, detail: Option<&PrDetail>) -> Outcome {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if let Some(comment_id) = self.confirm_delete.take() {
            match (k.code, detail) {
                (KeyCode::Char('y'), Some(d)) => ctx.mutate(key, &d.id, Mutation::DeleteComment { comment_id }),
                _ => ctx.info("kept"),
            }
            return Outcome::Handled;
        }
        if std::mem::take(&mut self.pending_g) {
            return match k.code {
                KeyCode::Char('g') => {
                    self.cursor = 0;
                    Outcome::Handled
                }
                KeyCode::Char('d') => self
                    .head_position()
                    .map_or(Outcome::Handled, |(p, l, c)| Outcome::Definition(p, l, c)),
                KeyCode::Char('r') => self
                    .head_position()
                    .map_or(Outcome::Handled, |(p, l, c)| Outcome::References(p, l, c)),
                _ => Outcome::Handled,
            };
        }
        let before = self.cursor;
        let outcome = self.on_motion_key(k, ctx, key, detail, ctrl);
        // Moved to another row: keep the column on screen, like vim's gj/gk on wrapped lines.
        if self.cursor != before
            && let Some(Row::Line(_, s)) = self.rows.get(self.cursor)
        {
            let w = self.seg_w();
            self.col = s * w + self.col % w;
        }
        outcome
    }

    fn on_motion_key(
        &mut self,
        k: KeyEvent,
        ctx: &mut Ctx,
        key: Key,
        detail: Option<&PrDetail>,
        ctrl: bool,
    ) -> Outcome {
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        match k.code {
            KeyCode::Char('e') if ctrl => {
                self.hide_list = !self.hide_list;
                self.focus_diff = true;
            }
            KeyCode::Char('<') => self.split.nudge(-4),
            KeyCode::Char('>') => self.split.nudge(4),
            KeyCode::Char('z') if alt => {
                self.wrap = !self.wrap;
                self.hscroll = 0;
                self.anchor = None; // its row moves
            }
            // With the list hidden, focus stays on the diff.
            KeyCode::Tab => self.focus_diff = !self.focus_diff || self.hide_list,
            KeyCode::Char(']') => self.select(self.sel + 1),
            KeyCode::Char('[') => self.select(self.sel.saturating_sub(1)),
            KeyCode::Char(' ') => self.toggle_viewed(ctx, key, detail),
            KeyCode::Char('z') => {
                if let Some(f) = self.files.get_mut(self.sel) {
                    f.folded = !f.folded;
                }
            }
            _ if !self.focus_diff => match k.code {
                KeyCode::Char('j') | KeyCode::Down => self.select(self.sel + 1),
                KeyCode::Char('k') | KeyCode::Up => self.select(self.sel.saturating_sub(1)),
                KeyCode::Char('g') => self.select(0),
                KeyCode::Char('G') => self.select(usize::MAX),
                KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => self.focus_diff = true,
                _ => return Outcome::Unhandled,
            },
            KeyCode::Esc if self.anchor.is_some() => self.anchor = None,
            KeyCode::Esc if !self.hide_list => self.focus_diff = false,
            KeyCode::Char('j') | KeyCode::Down => self.move_cursor(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_cursor(-1),
            KeyCode::Char('d') if ctrl => self.move_cursor(self.height as isize / 2),
            KeyCode::Char('u') if ctrl => self.move_cursor(-(self.height as isize) / 2),
            KeyCode::Char('h') | KeyCode::Left => self.col = self.col.saturating_sub(1),
            KeyCode::Char('l') | KeyCode::Right => {
                self.col = (self.col + 1).min(self.line_chars().len().saturating_sub(1))
            }
            KeyCode::Char('w') => self.col = word_motion(&self.line_chars(), self.col, true),
            KeyCode::Char('b') => self.col = word_motion(&self.line_chars(), self.col, false),
            KeyCode::Char('0') => self.col = 0,
            KeyCode::Char('$') => self.col = self.line_chars().len().saturating_sub(1),
            KeyCode::Char('g') => self.pending_g = true,
            KeyCode::Char('G') => self.cursor = self.rows.len().saturating_sub(1),
            KeyCode::Char('}') => self.jump_hunk(true),
            KeyCode::Char('{') => self.jump_hunk(false),
            KeyCode::Char('K') => {
                return self
                    .head_position()
                    .map_or(Outcome::Handled, |(p, l, c)| Outcome::Hover(p, l, c));
            }
            KeyCode::Char('v') => self.anchor = if self.anchor.is_some() { None } else { Some(self.cursor) },
            KeyCode::Char('c') => self.comment(ctx, key, detail),
            KeyCode::Char('a') => self.ask_claude(ctx, key, detail),
            KeyCode::Char('r') => {
                if let (Some(t), Some(d)) = (self.current_thread(), detail) {
                    let thread_id = d.review_threads.nodes[t].id.clone();
                    ctx.compose(
                        key,
                        &d.id,
                        Mutation::Reply {
                            thread_id,
                            body: String::new(),
                        },
                    );
                }
            }
            KeyCode::Char('D') => self.ask_delete(ctx, detail),
            KeyCode::Char('R') => {
                if let (Some(t), Some(d)) = (self.current_thread(), detail) {
                    let t = &d.review_threads.nodes[t];
                    ctx.mutate(
                        key,
                        &d.id,
                        Mutation::SetResolved {
                            thread_id: t.id.clone(),
                            resolved: !t.is_resolved,
                        },
                    );
                }
            }
            _ => return Outcome::Unhandled,
        }
        Outcome::Handled
    }

    /// Asks to confirm deleting the comment under the cursor.
    fn ask_delete(&mut self, ctx: &mut Ctx, detail: Option<&PrDetail>) {
        let (Some(Row::Thread(t, Some(c), _)), Some(d)) = (self.rows.get(self.cursor), detail) else {
            return ctx.info("put the cursor on a comment to delete it");
        };
        let comment = &d.review_threads.nodes[*t].comments.nodes[*c];
        if !comment.viewer_can_delete {
            return ctx.error("you can only delete your own comments");
        }
        let what = if comment.state == "PENDING" {
            "pending comment"
        } else {
            "published comment"
        };
        ctx.info(format!("delete this {what}? y to confirm, any other key keeps it"));
        self.confirm_delete = Some(comment.id.clone());
    }

    /// Marks viewed locally right away (GitHub sync is queued) and moves on to
    /// the next unviewed file, so a review can be ticked through quickly.
    fn toggle_viewed(&mut self, ctx: &mut Ctx, key: Key, detail: Option<&PrDetail>) {
        let (Some(d), Some(f)) = (detail, self.files.get_mut(self.sel)) else {
            return;
        };
        f.viewed = !f.viewed;
        let viewed = f.viewed;
        ctx.mutate(
            key,
            &d.id,
            Mutation::SetViewed {
                path: f.path.clone(),
                viewed,
            },
        );
        if viewed {
            let n = self.files.len();
            if let Some(next) = (1..n).map(|o| (self.sel + o) % n).find(|&i| !self.files[i].viewed) {
                self.select(next);
            }
        }
    }

    fn comment(&mut self, ctx: &mut Ctx, key: Key, detail: Option<&PrDetail>) {
        let Some(d) = detail else { return };
        let anchor_of = |row: usize, me: &Self| match me.rows.get(row) {
            Some(Row::Line(i, _)) => me.files[me.sel].lines[*i].anchor(),
            _ => None,
        };
        let Some((side, line)) = anchor_of(self.cursor, self) else {
            return ctx.error("put the cursor on a diff line to comment");
        };
        let start = self.anchor.take().and_then(|a| anchor_of(a, self));
        // GitHub ranges go top to bottom on one side.
        let (line, start_line) = match start {
            Some((s_side, s)) if s_side == side && s != line => (line.max(s), Some(line.min(s))),
            _ => (line, None),
        };
        let path = self.files[self.sel].path.clone();
        ctx.compose(
            key,
            &d.id,
            Mutation::AddThread {
                path,
                line,
                side,
                start_line,
                body: String::new(),
            },
        );
    }

    /// Sends the line, range or thread under the cursor to Claude Code, with a message.
    fn ask_claude(&mut self, ctx: &mut Ctx, key: Key, detail: Option<&PrDetail>) {
        let Some(f) = self.files.get(self.sel) else { return };
        let (a, b) = match self.anchor.take() {
            Some(a) => (a.min(self.cursor), a.max(self.cursor)),
            None => (self.cursor, self.cursor),
        };
        let mut picked: Vec<usize> = (a..=b)
            .filter_map(|r| match self.rows.get(r)? {
                Row::Line(i, _) => Some(*i),
                _ => None,
            })
            .collect();
        picked.dedup(); // a wrapped line has several rows
        let lines: Vec<&DiffLine> = picked.iter().filter_map(|&i| f.lines.get(i)).collect();
        let thread = self
            .current_thread()
            .zip(detail)
            .map(|(t, d)| &d.review_threads.nodes[t]);
        if lines.is_empty() && thread.is_none() {
            return ctx.error("put the cursor on a diff line or a thread to ask Claude");
        }
        let context = claude_context(ctx.repo(key.0), key.1, &f.path, &lines, thread);
        ctx.ask_claude(context);
    }

    /// A file selects it (or focuses the diff if already selected); the diff moves the cursor there.
    pub fn on_click(&mut self, pos: Position) {
        self.confirm_delete = None;
        self.pending_g = false;
        if let Some(i) = ui::row_at(self.list_area, self.list.offset(), pos).filter(|&i| i < self.files.len()) {
            self.focus_diff = i == self.sel;
            if i != self.sel {
                self.select(i);
            }
        } else if let Some(r) = ui::row_at(self.diff_area, self.scroll, pos).filter(|&r| r < self.rows.len()) {
            self.focus_diff = true;
            self.cursor = r;
            let seg = match self.rows[r] {
                Row::Line(_, s) => s,
                _ => 0,
            };
            let x = (pos.x - self.diff_area.x) as usize;
            let col = x.saturating_sub(GUTTER) + seg * self.code_w + self.hscroll;
            self.col = col.min(self.line_chars().len().saturating_sub(1));
        }
    }

    /// The border between the list and the diff.
    pub fn on_mouse(&mut self, m: MouseEvent) -> bool {
        !self.hide_list && self.split.on_mouse(m)
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect, detail: Option<&PrDetail>, hl: &Highlighter) {
        if self.hide_list {
            self.list_area = Rect::default();
            return self.render_diff(f, area, detail, hl);
        }
        let [list_area, diff_area] = self.split.areas(area, (area.width / 3).clamp(24, 56));
        self.render_list(f, list_area, detail);
        self.render_diff(f, diff_area, detail, hl);
        self.split.draw(f);
    }

    fn render_list(&mut self, f: &mut Frame, area: Rect, detail: Option<&PrDetail>) {
        let mut comments: HashMap<&str, usize> = HashMap::new();
        for t in detail
            .iter()
            .flat_map(|d| &d.review_threads.nodes)
            .filter(|t| !t.is_resolved)
        {
            *comments.entry(t.path.as_str()).or_default() += 1;
        }
        let items: Vec<ListItem> = self
            .files
            .iter()
            .map(|file| {
                let mark = if file.viewed { "✓ ".green() } else { "  ".into() };
                let status = match file.status.as_str() {
                    "added" => "A ".green(),
                    "removed" => "D ".red(),
                    "renamed" => "R ".yellow(),
                    _ => "M ".blue(),
                };
                let (dir, name) = file
                    .path
                    .rsplit_once('/')
                    .map_or(("", file.path.as_str()), |(d, n)| (d, n));
                let mut spans = vec![mark, status];
                if !dir.is_empty() {
                    spans.push(format!("{dir}/").dark_gray());
                }
                let name_style = if file.viewed || file.folded { DIM } else { Style::new() };
                spans.push(Span::styled(name.to_string(), name_style));
                if let Some(n) = comments.get(file.path.as_str()) {
                    spans.push(format!(" 💬{n}").yellow());
                }
                ListItem::new(Line::from(spans))
            })
            .collect();
        let title = format!("files {}/{} viewed", self.viewed_count(), self.files.len());
        self.list.select(Some(self.sel));
        self.list_area = area.inner(Margin::new(1, 1));
        let list = List::new(items)
            .highlight_style(SELECTED)
            .block(ui::block(title, !self.focus_diff));
        f.render_stateful_widget(list, area, &mut self.list);
    }

    fn build_rows(&self, detail: Option<&PrDetail>, hl: &Highlighter, width: usize) -> Vec<Row> {
        let Some(file) = self.files.get(self.sel) else {
            return Vec::new();
        };
        let mut rows = Vec::new();
        if !file.has_patch {
            rows.push(Row::Note(
                " no diff to show (binary file, or too large for GitHub's API)"
                    .dark_gray()
                    .into(),
            ));
        } else if file.folded {
            rows.push(Row::Note(" generated file folded, z to unfold".dark_gray().into()));
            return rows;
        }

        let threads: Vec<(usize, &crate::gh::model::Thread)> = detail
            .iter()
            .flat_map(|d| d.review_threads.nodes.iter().enumerate())
            .filter(|(_, t)| t.path == file.path)
            .collect();
        let thread_rows = |t: usize, th: &crate::gh::model::Thread, rows: &mut Vec<Row>| {
            for (c, line) in conversation::thread(th, hl) {
                for l in ui::wrap_comment(line, width.saturating_sub(GUTTER + 2)) {
                    rows.push(Row::Thread(t, c, l));
                }
            }
        };
        // Outdated threads have no line on the current diff: list them first.
        for (t, th) in threads.iter().filter(|(_, th)| th.line.is_none()) {
            thread_rows(*t, th, &mut rows);
        }
        let mut anchored: HashMap<(&str, u32), Vec<usize>> = HashMap::new();
        for (t, th) in &threads {
            if let Some(l) = th.line {
                anchored.entry((th.diff_side.as_str(), l)).or_default().push(*t);
            }
        }
        for (i, l) in file.lines.iter().enumerate() {
            let segs = l
                .text
                .replace('\t', "    ")
                .chars()
                .count()
                .div_ceil(self.seg_w())
                .max(1);
            rows.extend((0..segs).map(|s| Row::Line(i, s)));
            if let Some(ts) = l.anchor().and_then(|a| anchored.get(&a)) {
                for &t in ts {
                    thread_rows(t, &detail.unwrap().review_threads.nodes[t], &mut rows);
                }
            }
        }
        rows
    }

    fn render_diff(&mut self, f: &mut Frame, area: Rect, detail: Option<&PrDetail>, hl: &Highlighter) {
        let Some(file) = self.files.get_mut(self.sel) else {
            return f.render_widget(Paragraph::new(" no files").block(ui::block("diff", false)), area);
        };
        if file.hl.is_none() {
            let code = file.lines.iter().filter(|l| is_code(l.kind)).map(|l| l.text.as_str());
            let mut styled = hl.file(&file.path, code).into_iter();
            file.hl = Some(
                file.lines
                    .iter()
                    .map(|l| {
                        if is_code(l.kind) {
                            styled.next().unwrap_or_default()
                        } else {
                            Vec::new()
                        }
                    })
                    .collect(),
            );
        }
        let mut title = vec![format!(" {} ", file.path).bold()];
        if let Some(prev) = &file.prev {
            title.push(format!("← {prev} ").dark_gray());
        }
        title.push(format!("+{}", file.additions).green());
        title.push(format!(" -{} ", file.deletions).red());
        if file.viewed {
            title.push("✓ viewed ".green());
        }
        let block = ui::block("", self.focus_diff).title(Line::from(title));
        let inner = block.inner(area);
        f.render_widget(block, area);

        self.diff_area = inner;
        self.code_w = (inner.width as usize).saturating_sub(GUTTER).max(1);
        let on_line = match self.rows.get(self.cursor) {
            Some(Row::Line(i, _)) => Some(*i),
            _ => None,
        };
        self.rows = self.build_rows(detail, hl, inner.width as usize);
        self.height = inner.height as usize;
        // Rows move when wrapping is toggled or the width changes: stay on the
        // same line, on the segment holding the column.
        let seg = self.col / self.seg_w();
        if let Some(r) = on_line.and_then(|i| {
            self.rows
                .iter()
                .rposition(|r| matches!(r, Row::Line(j, s) if *j == i && *s <= seg))
        }) {
            self.cursor = r;
        }
        self.cursor = self.cursor.min(self.rows.len().saturating_sub(1));
        if !self.wrap && self.current_line().is_some() {
            let col = self.col.min(self.line_chars().len());
            if col < self.hscroll {
                self.hscroll = col;
            } else if col >= self.hscroll + self.code_w {
                self.hscroll = col + 1 - self.code_w;
            }
        }
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.cursor >= self.scroll + self.height {
            self.scroll = self.cursor + 1 - self.height;
        }
        let file = &self.files[self.sel];
        let styled = file.hl.as_deref().unwrap_or_default();
        let sel_range = self.anchor.map(|a| a.min(self.cursor)..=a.max(self.cursor));
        let lines: Vec<Line> = self
            .rows
            .iter()
            .enumerate()
            .skip(self.scroll)
            .take(self.height)
            .map(|(r, row)| {
                let is_cursor = self.focus_diff && r == self.cursor;
                let mut line = match row {
                    Row::Line(i, s) => {
                        let start = s * self.code_w + self.hscroll;
                        let cursor = is_cursor.then(|| self.col.saturating_sub(start));
                        diff_line(&file.lines[*i], &styled[*i], cursor, *s == 0, start, self.code_w)
                    }
                    Row::Thread(_, _, l) => {
                        let mut spans = vec![Span::raw(" ".repeat(GUTTER)), "│ ".fg(ui::ACCENT)];
                        spans.extend(l.spans.iter().cloned());
                        Line::from(spans).bg(THREAD_BG)
                    }
                    Row::Note(l) => l.clone(),
                };
                if sel_range.as_ref().is_some_and(|s| s.contains(&r)) {
                    line = line.patch_style(Style::new().bg(Color::Rgb(60, 60, 100)));
                } else if is_cursor {
                    line = line.patch_style(SELECTED);
                }
                line
            })
            .collect();
        f.render_widget(Paragraph::new(lines), inner);
    }
}

fn is_code(k: Kind) -> bool {
    matches!(k, Kind::Context | Kind::Add | Kind::Del)
}

fn num(n: Option<u32>) -> String {
    n.map_or("    ".into(), |n| format!("{n:>4}"))
}

/// The cells `start..start + len` of a diff line's code, behind its gutter
/// (blank on the continuation rows of a wrapped line).
fn diff_line(
    l: &DiffLine,
    code: &StyledLine,
    cursor: Option<usize>,
    first: bool,
    start: usize,
    len: usize,
) -> Line<'static> {
    let (marker, bg) = match l.kind {
        Kind::Add => ("+", Some(ADD_BG)),
        Kind::Del => ("-", Some(DEL_BG)),
        Kind::Hunk | Kind::Meta => {
            let mut spans = vec![Span::raw(" ".repeat(GUTTER))];
            spans.extend(slice(
                &[l.text.clone().fg(ui::ACCENT).add_modifier(Modifier::DIM)],
                start,
                len,
            ));
            return Line::from(spans);
        }
        Kind::Context => (" ", None),
    };
    let mut spans = if first {
        vec![
            Span::styled(format!("{} {} ", num(l.old), num(l.new)), DIM),
            Span::styled(
                format!("{marker} "),
                Style::new().fg(if l.kind == Kind::Add { Color::Green } else { Color::Red }),
            ),
        ]
    } else {
        vec![Span::raw(" ".repeat(GUTTER))]
    };
    let code = slice(code, start, len);
    spans.extend(match cursor {
        Some(c) => with_cursor(&code, c),
        None => code,
    });
    let line = Line::from(spans);
    match bg {
        Some(bg) => line.bg(bg),
        None => line,
    }
}

/// The cells `start..start + len` of a styled line.
fn slice(spans: &[Span<'static>], start: usize, len: usize) -> Vec<Span<'static>> {
    let end = start.saturating_add(len);
    let mut pos = 0;
    let mut out = Vec::new();
    for s in spans {
        let n = s.content.chars().count();
        let (a, b) = (start.max(pos), end.min(pos + n));
        if a < b {
            let text: String = s.content.chars().skip(a - pos).take(b - a).collect();
            out.push(Span::styled(text, s.style));
        }
        pos += n;
    }
    out
}

/// Next/previous word start from `col`, vim `w`/`b` style.
pub fn word_motion(chars: &[char], col: usize, forward: bool) -> usize {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    let mut c = col.min(chars.len());
    if forward {
        while c < chars.len() && word(chars[c]) {
            c += 1;
        }
        while c < chars.len() && !word(chars[c]) {
            c += 1;
        }
    } else {
        c = c.saturating_sub(1);
        while c > 0 && !word(chars[c]) {
            c -= 1;
        }
        while c > 0 && word(chars[c - 1]) {
            c -= 1;
        }
    }
    c.min(chars.len().saturating_sub(1))
}

/// Reverses the cell at display column `col`.
pub fn with_cursor(spans: &[Span<'static>], col: usize) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    let mut pos = 0;
    let mut placed = false;
    for s in spans {
        let n = s.content.chars().count();
        if !placed && col < pos + n {
            let chars: Vec<char> = s.content.chars().collect();
            let at = col - pos;
            out.push(Span::styled(chars[..at].iter().collect::<String>(), s.style));
            out.push(Span::styled(
                chars[at].to_string(),
                s.style.add_modifier(Modifier::REVERSED),
            ));
            out.push(Span::styled(chars[at + 1..].iter().collect::<String>(), s.style));
            placed = true;
        } else {
            out.push(s.clone());
        }
        pos += n;
    }
    if !placed {
        out.push(Span::styled(" ", Style::new().add_modifier(Modifier::REVERSED)));
    }
    out
}

/// Where a message to Claude is about: the PR, the place in it, its diff lines and thread.
fn claude_context(repo: &str, pr: u64, path: &str, lines: &[&DiffLine], thread: Option<&Thread>) -> String {
    // One side's numbers: a range mixing old and new ones points nowhere.
    let new: Vec<u32> = lines.iter().filter_map(|l| l.new).collect();
    let nums = match new.is_empty() {
        true => lines.iter().filter_map(|l| l.old).collect(),
        false => new,
    };
    let place = match (nums.first(), nums.last()) {
        (Some(a), Some(b)) if a != b => format!("{path}:{a}-{b}"),
        (Some(a), _) => format!("{path}:{a}"),
        _ => match thread.and_then(|t| t.line.or(t.original_line)) {
            Some(n) => format!("{path}:{n}"),
            None => path.to_string(),
        },
    };
    let mut out = format!("About PR #{pr} of {repo}, `{place}`.\n");
    if !lines.is_empty() {
        // Longer than any backtick run in the code, so a ``` there can't close it.
        let ticks = lines.iter().map(|l| longest_run(&l.text, '`')).max().unwrap_or(0);
        let fence = "`".repeat(ticks.max(2) + 1);
        out.push_str(&format!("Diff (from the PR):\n{fence}diff\n"));
        for l in lines {
            let mark = match l.kind {
                Kind::Add => "+",
                Kind::Del => "-",
                Kind::Context => " ",
                Kind::Hunk | Kind::Meta => "",
            };
            out.push_str(&format!("{mark}{}\n", l.text));
        }
        out.push_str(&format!("{fence}\n"));
    }
    if let Some(t) = thread {
        out.push_str("Review thread there (written by others, not by me):\n");
        for c in &t.comments.nodes {
            let who = c.author.as_ref().map_or("ghost", |a| a.login.as_str());
            out.push_str(&format!("> @{who}: {}\n", c.body.replace('\n', "\n> ")));
        }
    }
    out.push_str("\nMy message:");
    out
}

fn longest_run(s: &str, c: char) -> usize {
    s.split(|x| x != c).map(str::len).max().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_cell() {
        let spans = vec![Span::raw("ab"), Span::raw("cd")];
        let out = with_cursor(&spans, 2);
        assert_eq!(out.iter().map(|s| s.content.as_ref()).collect::<String>(), "abcd");
        assert!(
            out.iter()
                .any(|s| s.content == "c" && s.style.add_modifier.contains(Modifier::REVERSED))
        );
        assert_eq!(with_cursor(&spans, 9).last().unwrap().content, " ");
    }

    #[test]
    fn slices() {
        let spans = vec![Span::raw("abc"), Span::raw("def")];
        let text = |v: Vec<Span>| v.iter().map(|s| s.content.to_string()).collect::<String>();
        assert_eq!(text(slice(&spans, 2, 3)), "cde");
        assert_eq!(text(slice(&spans, 4, usize::MAX)), "ef");
        assert_eq!(text(slice(&spans, 9, 3)), "");
    }

    #[test]
    fn context_for_claude() {
        let lines = diff::parse("@@ -1,2 +1,2 @@\n-old\n+new\n same");
        let picked: Vec<&DiffLine> = lines[1..].iter().collect();
        let out = claude_context("o/r", 7, "a.rs", &picked, None);
        let want =
            "About PR #7 of o/r, `a.rs:1-2`.\nDiff (from the PR):\n```diff\n-old\n+new\n same\n```\n\nMy message:";
        assert_eq!(out, want);
        // Old-side numbers only when nothing has a new one; fences outgrow the code's.
        let lines = diff::parse("@@ -4,2 +4,0 @@\n-a\n-````x");
        let picked: Vec<&DiffLine> = lines[1..].iter().collect();
        let out = claude_context("o/r", 7, "a.md", &picked, None);
        assert!(out.contains("`a.md:4-5`") && out.contains("\n`````diff\n") && out.contains("\n`````\n"));
    }
}

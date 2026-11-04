//! "Files changed": file list + diff with inline review threads.

use super::{Outcome, conversation};
use crate::app::ui::{self, DIM, SELECTED};
use crate::app::{Ctx, Key};
use crate::diff::{self, DiffLine, Kind};
use crate::gh::model::{ChangedFile, PrDetail};
use crate::gh::mutation::Mutation;
use crate::highlight::{Highlighter, StyledLine};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
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
    "Cargo.lock", "package-lock.json", "yarn.lock", "pnpm-lock.yaml", "flake.lock", "poetry.lock", "uv.lock",
    "go.sum", "Gemfile.lock", "composer.lock", "bun.lockb",
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
    Line(usize),
    /// A rendered line of review thread `.0` (index in `review_threads`).
    Thread(usize, Line<'static>),
    Note(Line<'static>),
}

pub struct FilesView {
    pub files: Vec<FileEntry>,
    pub sel: usize,
    focus_diff: bool,
    cursor: usize,
    pub col: usize,
    scroll: usize,
    /// Start row of a visual (range) selection.
    anchor: Option<usize>,
    pending_g: bool,
    /// Rows as last rendered, so keys act on what is on screen.
    rows: Vec<Row>,
    height: usize,
    list: ListState,
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
            anchor: None,
            pending_g: false,
            rows: Vec::new(),
            height: 20,
            list: ListState::default(),
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
        self.col = 0;
        self.anchor = None;
    }

    fn current_line(&self) -> Option<&DiffLine> {
        match self.rows.get(self.cursor)? {
            Row::Line(i) => self.files.get(self.sel)?.lines.get(*i),
            _ => None,
        }
    }

    fn current_thread(&self) -> Option<usize> {
        match self.rows.get(self.cursor)? {
            Row::Thread(t, _) => Some(*t),
            _ => None,
        }
    }

    fn line_chars(&self) -> Vec<char> {
        self.current_line().map(|l| l.text.replace('\t', "    ").chars().collect()).unwrap_or_default()
    }

    fn move_cursor(&mut self, delta: isize) {
        let max = self.rows.len().saturating_sub(1);
        self.cursor = self.cursor.saturating_add_signed(delta).min(max);
    }

    fn jump_hunk(&mut self, forward: bool) {
        let Some(f) = self.files.get(self.sel) else { return };
        let is_hunk = |r: &Row| matches!(r, Row::Line(i) if f.lines[*i].kind == Kind::Hunk);
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
        if std::mem::take(&mut self.pending_g) {
            return match k.code {
                KeyCode::Char('g') => {
                    self.cursor = 0;
                    Outcome::Handled
                }
                KeyCode::Char('d') => self.head_position().map_or(Outcome::Handled, |(p, l, c)| Outcome::Definition(p, l, c)),
                KeyCode::Char('r') => self.head_position().map_or(Outcome::Handled, |(p, l, c)| Outcome::References(p, l, c)),
                _ => Outcome::Handled,
            };
        }
        match k.code {
            KeyCode::Tab => self.focus_diff = !self.focus_diff,
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
            KeyCode::Esc => self.focus_diff = false,
            KeyCode::Char('j') | KeyCode::Down => self.move_cursor(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_cursor(-1),
            KeyCode::Char('d') if ctrl => self.move_cursor(self.height as isize / 2),
            KeyCode::Char('u') if ctrl => self.move_cursor(-(self.height as isize) / 2),
            KeyCode::Char('h') | KeyCode::Left => self.col = self.col.saturating_sub(1),
            KeyCode::Char('l') | KeyCode::Right => self.col = (self.col + 1).min(self.line_chars().len().saturating_sub(1)),
            KeyCode::Char('w') => self.col = word_motion(&self.line_chars(), self.col, true),
            KeyCode::Char('b') => self.col = word_motion(&self.line_chars(), self.col, false),
            KeyCode::Char('0') => self.col = 0,
            KeyCode::Char('$') => self.col = self.line_chars().len().saturating_sub(1),
            KeyCode::Char('g') => self.pending_g = true,
            KeyCode::Char('G') => self.cursor = self.rows.len().saturating_sub(1),
            KeyCode::Char('}') => self.jump_hunk(true),
            KeyCode::Char('{') => self.jump_hunk(false),
            KeyCode::Char('K') => {
                return self.head_position().map_or(Outcome::Handled, |(p, l, c)| Outcome::Hover(p, l, c));
            }
            KeyCode::Char('v') => self.anchor = if self.anchor.is_some() { None } else { Some(self.cursor) },
            KeyCode::Char('c') => self.comment(ctx, key, detail),
            KeyCode::Char('r') => {
                if let (Some(t), Some(d)) = (self.current_thread(), detail) {
                    let thread_id = d.review_threads.nodes[t].id.clone();
                    ctx.compose(key, &d.id, Mutation::Reply { thread_id, body: String::new() });
                }
            }
            KeyCode::Char('R') => {
                if let (Some(t), Some(d)) = (self.current_thread(), detail) {
                    let t = &d.review_threads.nodes[t];
                    ctx.mutate(key, &d.id, Mutation::SetResolved { thread_id: t.id.clone(), resolved: !t.is_resolved });
                }
            }
            _ => return Outcome::Unhandled,
        }
        Outcome::Handled
    }

    /// Marks viewed locally right away (GitHub sync is queued) and moves on to
    /// the next unviewed file, so a review can be ticked through quickly.
    fn toggle_viewed(&mut self, ctx: &mut Ctx, key: Key, detail: Option<&PrDetail>) {
        let (Some(d), Some(f)) = (detail, self.files.get_mut(self.sel)) else { return };
        f.viewed = !f.viewed;
        let viewed = f.viewed;
        ctx.mutate(key, &d.id, Mutation::SetViewed { path: f.path.clone(), viewed });
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
            Some(Row::Line(i)) => me.files[me.sel].lines[*i].anchor(),
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
        ctx.compose(key, &d.id, Mutation::AddThread { path, line, side, start_line, body: String::new() });
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect, detail: Option<&PrDetail>, hl: &Highlighter) {
        let list_w = (area.width / 3).clamp(24, 56);
        let [list_area, diff_area] = Layout::horizontal([Constraint::Length(list_w), Constraint::Fill(1)]).areas(area);
        self.render_list(f, list_area, detail);
        self.render_diff(f, diff_area, detail, hl);
    }

    fn render_list(&mut self, f: &mut Frame, area: Rect, detail: Option<&PrDetail>) {
        let mut comments: HashMap<&str, usize> = HashMap::new();
        for t in detail.iter().flat_map(|d| &d.review_threads.nodes).filter(|t| !t.is_resolved) {
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
                let (dir, name) = file.path.rsplit_once('/').map_or(("", file.path.as_str()), |(d, n)| (d, n));
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
        let list = List::new(items).highlight_style(SELECTED).block(ui::block(title, !self.focus_diff));
        f.render_stateful_widget(list, area, &mut self.list);
    }

    fn build_rows(&self, detail: Option<&PrDetail>, hl: &Highlighter, width: usize) -> Vec<Row> {
        let Some(file) = self.files.get(self.sel) else { return Vec::new() };
        let mut rows = Vec::new();
        if !file.has_patch {
            rows.push(Row::Note(" no diff to show (binary file, or too large for GitHub's API)".dark_gray().into()));
        } else if file.folded {
            rows.push(Row::Note(" generated file folded, z to unfold".dark_gray().into()));
            return rows;
        }

        let threads: Vec<(usize, &crate::gh::model::Thread)> =
            detail.iter().flat_map(|d| d.review_threads.nodes.iter().enumerate()).filter(|(_, t)| t.path == file.path).collect();
        let thread_rows = |t: usize, th: &crate::gh::model::Thread, rows: &mut Vec<Row>| {
            for line in conversation::thread(th, hl) {
                for l in wrap(line, width.saturating_sub(GUTTER + 2)) {
                    rows.push(Row::Thread(t, l));
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
            rows.push(Row::Line(i));
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
            file.hl = Some(file.lines.iter().map(|l| if is_code(l.kind) { styled.next().unwrap_or_default() } else { Vec::new() }).collect());
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

        self.rows = self.build_rows(detail, hl, inner.width as usize);
        self.height = inner.height as usize;
        self.cursor = self.cursor.min(self.rows.len().saturating_sub(1));
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
                    Row::Line(i) => diff_line(&file.lines[*i], &styled[*i], is_cursor.then_some(self.col)),
                    Row::Thread(_, l) => {
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

fn diff_line(l: &DiffLine, code: &StyledLine, cursor: Option<usize>) -> Line<'static> {
    let (marker, bg) = match l.kind {
        Kind::Add => ("+", Some(ADD_BG)),
        Kind::Del => ("-", Some(DEL_BG)),
        Kind::Hunk | Kind::Meta => {
            return Line::from(vec![Span::raw(" ".repeat(GUTTER)), l.text.clone().fg(ui::ACCENT).add_modifier(Modifier::DIM)]);
        }
        Kind::Context => (" ", None),
    };
    let mut spans = vec![
        Span::styled(format!("{} {} ", num(l.old), num(l.new)), DIM),
        Span::styled(format!("{marker} "), Style::new().fg(if l.kind == Kind::Add { Color::Green } else { Color::Red })),
    ];
    spans.extend(match cursor {
        Some(c) => with_cursor(code, c),
        None => code.clone(),
    });
    let line = Line::from(spans);
    match bg {
        Some(bg) => line.bg(bg),
        None => line,
    }
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
            out.push(Span::styled(chars[at].to_string(), s.style.add_modifier(Modifier::REVERSED)));
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

/// Word-wraps a styled line to `width` columns.
pub fn wrap(line: Line<'static>, width: usize) -> Vec<Line<'static>> {
    let width = width.max(10);
    let mut out = vec![Vec::new()];
    let mut used = 0;
    for span in line.spans {
        for word in span.content.split_inclusive(' ') {
            let w = word.chars().count();
            if used + w > width && used > 0 {
                out.push(Vec::new());
                used = 0;
            }
            // Words longer than a line are hard-split.
            let mut chars: Vec<char> = word.chars().collect();
            while chars.len() > width {
                let head: String = chars.drain(..width).collect();
                out.last_mut().unwrap().push(Span::styled(head, span.style));
                out.push(Vec::new());
            }
            used += chars.len();
            out.last_mut().unwrap().push(Span::styled(chars.into_iter().collect::<String>(), span.style));
        }
    }
    out.into_iter().map(Line::from).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(l: &Line) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn wraps_words() {
        let lines = wrap(Line::from("aaaa bbbb cccc dddddddddddddddddddddd"), 10);
        let t: Vec<_> = lines.iter().map(text).collect();
        assert_eq!(t, vec!["aaaa bbbb ", "cccc ", "dddddddddd", "dddddddddd", "dd"]);
    }

    #[test]
    fn cursor_cell() {
        let spans = vec![Span::raw("ab"), Span::raw("cd")];
        let out = with_cursor(&spans, 2);
        assert_eq!(out.iter().map(|s| s.content.as_ref()).collect::<String>(), "abcd");
        assert!(out.iter().any(|s| s.content == "c" && s.style.add_modifier.contains(Modifier::REVERSED)));
        assert_eq!(with_cursor(&spans, 9).last().unwrap().content, " ");
    }
}

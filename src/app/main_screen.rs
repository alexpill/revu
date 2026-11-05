//! Home: open PRs and issues across GitHub, grouped by repo, most pressing first.

use super::issue::IssueScreen;
use super::pr::PrScreen;
use super::ui::{self, DIM, SELECTED};
use super::{Action, Ctx, Msg, Screen};
use crate::gh::{self, model::*};
use anyhow::Result;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Margin, Position, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState, Tabs};
use std::collections::HashSet;

pub const HELP: &[(&str, &str)] = &[
    ("1-3 H/L Tab", "tabs: activity, pull requests, issues"),
    ("v", "view: involving me / pinned repos / everything you can access"),
    ("j/k g/G", "move"),
    ("Enter", "open PR / issue, or fold a repo"),
    ("click / wheel", "select (again to open) / move"),
    ("r a m d", "review requested / assigned / mine / drafts (again: all)"),
    ("f", "cycle the filter"),
    ("/", "filter by text"),
    ("c", "clone the repo (remembered for local reads and LSP)"),
    ("o", "open in browser"),
    ("Ctrl-r", "refresh"),
    ("q", "quit"),
];

const TABS: [&str; 3] = ["activity", "pull requests", "issues"];

#[derive(Clone, Copy, PartialEq)]
enum View {
    Me,
    Pinned,
    All,
}

impl View {
    const ALL: [View; 3] = [View::Me, View::Pinned, View::All];

    fn label(self) -> &'static str {
        match self {
            View::Me => "involving me",
            View::Pinned => "pinned repos",
            View::All => "everything",
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq)]
enum Filter {
    #[default]
    All,
    Review,
    Assigned,
    Mine,
    Draft,
}

impl Filter {
    const ALL: [Filter; 5] = [
        Filter::All,
        Filter::Review,
        Filter::Assigned,
        Filter::Mine,
        Filter::Draft,
    ];

    fn from_key(c: char) -> Option<Filter> {
        Some(match c {
            'r' => Filter::Review,
            'a' => Filter::Assigned,
            'm' => Filter::Mine,
            'd' => Filter::Draft,
            _ => return None,
        })
    }

    fn label(self) -> &'static str {
        match self {
            Filter::All => "all",
            Filter::Review => "review requested",
            Filter::Assigned => "assigned",
            Filter::Mine => "mine",
            Filter::Draft => "drafts",
        }
    }

    fn keep(self, item: &Item, me: &str) -> bool {
        match (self, item) {
            (Filter::All, _) => true,
            (Filter::Review, Item::Pr(p)) => p.review_requested(me),
            (Filter::Draft, Item::Pr(p)) => p.is_draft,
            (Filter::Assigned, Item::Pr(p)) => p.assigned_to(me),
            (Filter::Assigned, Item::Issue(i)) => i.assignees.nodes.iter().any(|a| a.login == me),
            (Filter::Mine, Item::Pr(p)) => p.author_is(me),
            (Filter::Mine, Item::Issue(i)) => i.author.as_ref().is_some_and(|a| a.login == me),
            _ => false,
        }
    }
}

/// What the viewer has to do with an item, most pressing first.
fn why(item: &Item, me: &str) -> (u8, &'static str) {
    let red = |p: &PrSummary| matches!(p.ci(), Some("FAILURE" | "ERROR"));
    match item {
        Item::Pr(p) if p.review_requested(me) => (0, "review"),
        Item::Pr(p) if p.author_is(me) && p.review_decision.as_deref() == Some("CHANGES_REQUESTED") => (1, "changes"),
        Item::Pr(p) if p.author_is(me) && red(p) => (1, "ci red"),
        _ if Filter::Assigned.keep(item, me) => (2, "assigned"),
        _ if Filter::Mine.keep(item, me) => (3, "author"),
        _ => (4, ""),
    }
}

struct Entry {
    repo: usize,
    item: Item,
}

/// Indices of `keep`-ing entries by repo: pinned repos first (in config
/// order), then the repo with the most pressing item, then the most recent.
/// Inside a repo, most pressing then most recent. `entries` are newest first.
fn group(
    entries: &[Entry],
    keep: impl Fn(&Entry) -> bool,
    me: &str,
    pinned: impl Fn(usize) -> Option<usize>,
) -> Vec<(usize, Vec<usize>)> {
    let mut groups: Vec<(usize, Vec<usize>)> = Vec::new();
    for (i, e) in entries.iter().enumerate().filter(|(_, e)| keep(e)) {
        match groups.iter_mut().find(|g| g.0 == e.repo) {
            Some(g) => g.1.push(i),
            None => groups.push((e.repo, vec![i])),
        }
    }
    // Stable sorts: ties keep the newest-first order.
    let rank = |i: &usize| why(&entries[*i].item, me).0;
    for g in &mut groups {
        g.1.sort_by_key(rank);
    }
    groups.sort_by_key(|g| (pinned(g.0).unwrap_or(usize::MAX), g.1.iter().map(rank).min()));
    groups
}

enum Line_ {
    /// Repo index, and how many PRs and issues it lists (folded or not).
    Repo(usize, usize, usize),
    Entry(usize),
}

pub struct MainScreen {
    tab: usize,
    view: View,
    /// Per view, newest first; `None` until loaded.
    data: [Option<Result<Vec<Entry>, String>>; 3],
    filter: Filter,
    text: String,
    folded: HashSet<usize>,
    sel: TableState,
    /// Text being typed after `/`.
    input: Option<String>,
    /// As last rendered, for clicks.
    tab_rects: Vec<Rect>,
    table_area: Rect,
}

impl MainScreen {
    pub fn new(ctx: &mut Ctx) -> Self {
        let mut s = Self::empty();
        s.load(ctx);
        s
    }

    /// Unloaded: nothing is searched until `load` (for `--pr`, never).
    pub fn empty() -> Self {
        Self {
            tab: 0,
            view: View::Me,
            data: [None, None, None],
            filter: Filter::All,
            text: String::new(),
            folded: HashSet::new(),
            sel: TableState::default().with_selected(0),
            input: None,
            tab_rects: Vec::new(),
            table_area: Rect::default(),
        }
    }

    fn load(&mut self, ctx: &mut Ctx) {
        let view = self.view;
        let pinned: Vec<String> = ctx.cfg.repos.iter().map(|r| format!("repo:{}", r.name)).collect();
        ctx.spawn(move || {
            let scopes = || -> Result<Vec<String>> {
                let me = "involves:@me".to_string();
                let pinned = (!pinned.is_empty()).then(|| pinned.join(" "));
                Ok(match view {
                    View::Me => vec![me],
                    View::Pinned => pinned.into_iter().collect(),
                    View::All => {
                        let orgs = gh::orgs()?.into_iter().map(|o| format!("org:{o}"));
                        let mine = std::iter::once("user:@me".to_string()).chain(orgs).collect::<Vec<_>>();
                        [Some(me), Some(mine.join(" ")), pinned].into_iter().flatten().collect()
                    }
                })
            };
            Msg::Activity(view as usize, scopes().and_then(|s| gh::search(&s)))
        });
    }

    pub fn on_activity(&mut self, view: usize, r: Result<Activity>, ctx: &mut Ctx) {
        self.data[view] = Some(match r {
            Ok(a) => {
                if !a.viewer.is_empty() {
                    ctx.viewer = a.viewer;
                }
                let entries = a.items.into_iter().map(|(repo, item)| Entry {
                    repo: ctx.repo_index(&repo),
                    item,
                });
                Ok(entries.collect())
            }
            Err(e) => Err(e.to_string()),
        });
    }

    fn entries(&self) -> Option<&Vec<Entry>> {
        self.data[self.view as usize].as_ref()?.as_ref().ok()
    }

    /// Visible rows: repo headers and, unless folded, their items.
    fn lines(&self, ctx: &Ctx) -> Vec<Line_> {
        let Some(entries) = self.entries() else {
            return Vec::new();
        };
        let me = ctx.viewer.as_str();
        let needle = self.text.to_lowercase();
        let keep = |e: &Entry| {
            let kind = matches!((self.tab, &e.item), (0, _) | (1, Item::Pr(_)) | (2, Item::Issue(_)));
            kind && self.filter.keep(&e.item, me) && (needle.is_empty() || haystack(e, ctx).contains(&needle))
        };
        let pinned = |repo: usize| ctx.pinned(ctx.repo(repo));
        let mut out = Vec::new();
        for (repo, items) in group(entries, keep, me, pinned) {
            let prs = items
                .iter()
                .filter(|i| matches!(entries[**i].item, Item::Pr(_)))
                .count();
            out.push(Line_::Repo(repo, prs, items.len() - prs));
            if !self.folded.contains(&repo) {
                out.extend(items.into_iter().map(Line_::Entry));
            }
        }
        out
    }

    fn selected(&self, ctx: &Ctx) -> Option<Line_> {
        let sel = self.sel.selected()?;
        self.lines(ctx).into_iter().nth(sel)
    }

    fn move_to(&mut self, row: usize, ctx: &Ctx) {
        let n = self.lines(ctx).len();
        self.sel.select(Some(row.min(n.saturating_sub(1))));
    }

    pub fn on_key(&mut self, k: KeyEvent, ctx: &mut Ctx) -> Action {
        if let Some(input) = &mut self.input {
            match k.code {
                KeyCode::Esc | KeyCode::Enter => self.input = None,
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Char(c) => input.push(c),
                _ => {}
            }
            // Filtering is live while typing.
            if let Some(text) = &self.input {
                self.text = text.clone();
                self.sel.select(Some(0));
            }
            return Action::None;
        }

        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let row = self.sel.selected().unwrap_or(0);
        match k.code {
            KeyCode::Char('q') => return Action::Quit,
            KeyCode::Esc => self.text.clear(),
            KeyCode::Char('?') => return Action::Help,
            KeyCode::Char('r') if ctrl => self.load(ctx),
            KeyCode::Tab | KeyCode::Char('L') => self.set_tab((self.tab + 1) % TABS.len()),
            KeyCode::BackTab | KeyCode::Char('H') => self.set_tab((self.tab + TABS.len() - 1) % TABS.len()),
            KeyCode::Char(c @ '1'..='3') => self.set_tab(c as usize - '1' as usize),
            KeyCode::Char('v') => {
                let pos = View::ALL.iter().position(|v| *v == self.view).unwrap_or(0);
                self.view = View::ALL[(pos + 1) % View::ALL.len()];
                if self.data[self.view as usize].is_none() {
                    self.load(ctx);
                }
                self.sel.select(Some(0));
            }
            KeyCode::Char('j') | KeyCode::Down => self.move_to(row + 1, ctx),
            KeyCode::Char('k') | KeyCode::Up => self.move_to(row.saturating_sub(1), ctx),
            KeyCode::Char('g') => self.move_to(0, ctx),
            KeyCode::Char('G') => self.move_to(usize::MAX, ctx),
            KeyCode::Char('f') => {
                let pos = Filter::ALL.iter().position(|x| *x == self.filter).unwrap_or(0);
                self.filter = Filter::ALL[(pos + 1) % Filter::ALL.len()];
                self.sel.select(Some(0));
            }
            KeyCode::Char(c) if Filter::from_key(c).is_some() => {
                let want = Filter::from_key(c).unwrap_or_default();
                self.filter = if self.filter == want { Filter::All } else { want };
                self.sel.select(Some(0));
            }
            KeyCode::Char('/') => self.input = Some(self.text.clone()),
            KeyCode::Char('c') => match self.selected(ctx) {
                Some(Line_::Repo(repo, ..)) => ctx.clone_repo(repo),
                Some(Line_::Entry(i)) => {
                    let repo = self.entries().map_or(0, |e| e[i].repo);
                    ctx.clone_repo(repo);
                }
                None => {}
            },
            KeyCode::Char('o') => {
                if let Some((_, key)) = self.selected_item(ctx) {
                    ctx.browse(key);
                }
            }
            KeyCode::Enter => match self.selected(ctx) {
                Some(Line_::Repo(repo, ..)) => {
                    if !self.folded.remove(&repo) {
                        self.folded.insert(repo);
                    }
                }
                _ => match self.selected_item(ctx) {
                    Some((false, key)) => return Action::Push(Screen::Issue(IssueScreen::open(key, ctx))),
                    Some((true, key)) => return Action::Push(Screen::Pr(Box::new(PrScreen::open(key, ctx)))),
                    None => {}
                },
            },
            _ => {}
        }
        Action::None
    }

    fn set_tab(&mut self, t: usize) {
        self.tab = t;
        self.sel.select(Some(0));
    }

    /// The PR or issue under the cursor, as (is_pr, key).
    fn selected_item(&self, ctx: &Ctx) -> Option<(bool, super::Key)> {
        let Line_::Entry(i) = self.selected(ctx)? else {
            return None;
        };
        let e = &self.entries()?[i];
        Some((matches!(e.item, Item::Pr(_)), (e.repo, e.item.number())))
    }

    /// A tab switches to it, a row selects it, the selected row opens.
    pub fn on_click(&mut self, pos: Position, ctx: &mut Ctx) -> Action {
        if self.input.is_some() {
            return Action::None;
        }
        if let Some(t) = self.tab_rects.iter().position(|r| r.contains(pos)) {
            self.set_tab(t);
            return Action::None;
        }
        let n = self.lines(ctx).len();
        match ui::row_at(self.table_area, self.sel.offset(), pos).filter(|&r| r < n) {
            Some(r) if self.sel.selected() == Some(r) => self.on_key(KeyCode::Enter.into(), ctx),
            Some(r) => {
                self.sel.select(Some(r));
                Action::None
            }
            None => Action::None,
        }
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let [tabs_area, body] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);
        let entries = self.entries();
        let titles: Vec<Line> = TABS
            .iter()
            .enumerate()
            .map(|(t, name)| {
                let n = entries.map(|es| match t {
                    1 => es.iter().filter(|e| matches!(e.item, Item::Pr(_))).count(),
                    2 => es.iter().filter(|e| matches!(e.item, Item::Issue(_))).count(),
                    _ => es.len(),
                });
                let n = n.map(|n| format!(" {n}")).unwrap_or_default();
                Line::from(vec![format!(" {} {name}", t + 1).into(), n.dark_gray(), " ".into()])
            })
            .collect();
        self.tab_rects = ui::tab_rects(&titles, tabs_area);
        self.table_area = Rect::default();
        f.render_widget(
            Tabs::new(titles)
                .select(self.tab)
                .highlight_style(Style::new().fg(ui::ACCENT).bold().reversed())
                .divider(""),
            tabs_area,
        );

        match &self.data[self.view as usize] {
            None => f.render_widget(Paragraph::new(" loading…").dark_gray(), body),
            Some(Err(e)) => f.render_widget(Paragraph::new(format!(" {e}")).red(), body),
            Some(Ok(_)) if self.view == View::Pinned && ctx.cfg.repos.is_empty() => {
                let msg = format!(
                    " no pinned repos: add [[repos]] to {}, or run revu from a clone",
                    crate::config::config_path().display()
                );
                f.render_widget(Paragraph::new(msg).yellow(), body);
            }
            Some(Ok(_)) => self.render_list(f, body, ctx),
        }
    }

    fn render_list(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let me = ctx.viewer.as_str();
        let lines = self.lines(ctx);
        let entries = self.entries().map(Vec::as_slice).unwrap_or_default();
        let rows: Vec<Row> = lines
            .iter()
            .map(|l| match l {
                Line_::Repo(repo, prs, issues) => repo_row(*repo, *prs, *issues, self.folded.contains(repo), ctx),
                Line_::Entry(i) => item_row(&entries[*i].item, me),
            })
            .collect();
        let mut title = format!("{} · {}", self.view.label(), self.filter.label());
        if !self.text.is_empty() {
            title += &format!(" · /{}", self.text);
        }
        title += " · v view · r a m d filters · c clone";
        let widths = [
            Constraint::Length(3),
            Constraint::Length(6),
            Constraint::Fill(1),
            Constraint::Length(8),
            Constraint::Length(16),
            Constraint::Length(4),
        ];
        let table = Table::new(rows, widths)
            .row_highlight_style(SELECTED)
            .block(ui::block(title.clone(), true));
        let area = self.render_input(f, area);
        if lines.is_empty() {
            let hint = Paragraph::new(" nothing open here · v: another view · f: another filter").dark_gray();
            return f.render_widget(hint.block(ui::block(title, true)), area);
        }
        self.table_area = area.inner(Margin::new(1, 1));
        f.render_stateful_widget(table, area, &mut self.sel);
    }

    /// Draws the `/` prompt (if active) at the bottom; returns the remaining area.
    fn render_input(&self, f: &mut Frame, area: Rect) -> Rect {
        let Some(input) = &self.input else { return area };
        let [rest, line] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area);
        f.render_widget(
            Paragraph::new(Line::from(vec!["/".cyan(), input.clone().into(), "█".dark_gray()])),
            line,
        );
        rest
    }
}

fn haystack(e: &Entry, ctx: &Ctx) -> String {
    let (title, author, branch) = match &e.item {
        Item::Pr(p) => (&p.title, &p.author, p.head_ref_name.as_str()),
        Item::Issue(i) => (&i.title, &i.author, ""),
    };
    let author = ui::login(author);
    format!("{} #{} {title} {author} {branch}", ctx.repo(e.repo), e.item.number()).to_lowercase()
}

/// `▾ owner/name ★ 2 PR · 1 issue · local`, counting the items listed under it.
fn repo_row(repo: usize, prs: usize, issues: usize, folded: bool, ctx: &Ctx) -> Row<'static> {
    let mut spans: Vec<Span> = vec![
        if folded { "▸ " } else { "▾ " }.into(),
        ctx.repo(repo).to_string().bold(),
    ];
    if ctx.pinned(ctx.repo(repo)).is_some() {
        spans.push(" ★".yellow());
    }
    let mut counts = Vec::new();
    if prs > 0 {
        counts.push(format!("{prs} PR"));
    }
    if issues > 0 {
        counts.push(format!("{issues} issue{}", if issues > 1 { "s" } else { "" }));
    }
    if ctx.sources[repo].local.is_some() {
        counts.push("local".into());
    }
    if !counts.is_empty() {
        spans.push(format!("  {}", counts.join(" · ")).dark_gray());
    }
    Row::new([Cell::from(Line::from(spans)).column_span(6)])
}

fn item_row(item: &Item, me: &str) -> Row<'static> {
    let (_, why) = why(item, me);
    let why = match why {
        "review" | "changes" | "ci red" => why.yellow(),
        _ => why.dark_gray(),
    };
    let (glyph, title, author, updated) = match item {
        Item::Pr(p) => (ui::ci(p.ci()), pr_title(p), &p.author, &p.updated_at),
        Item::Issue(i) => {
            let labels = i.labels.nodes.iter().map(|l| format!(" {}", l.name).dark_gray());
            let title = Line::from_iter(std::iter::once(Span::from(i.title.clone())).chain(labels));
            ("●".green(), title, &i.author, &i.updated_at)
        }
    };
    Row::new(vec![
        Cell::from(Line::from(vec!["  ".into(), glyph])),
        Cell::from(format!("#{}", item.number())).style(DIM),
        Cell::from(title),
        Cell::from(why),
        Cell::from(ui::login(author)).style(DIM),
        Cell::from(ui::ago(updated)).style(DIM),
    ])
}

fn pr_title(pr: &PrSummary) -> Line<'static> {
    if pr.is_draft {
        Line::from(vec!["draft ".dark_gray().italic(), pr.title.clone().dark_gray()])
    } else {
        Line::from(pr.title.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr(n: u64, author: &str, review_from: &[&str]) -> Item {
        let reviewers: Vec<_> = review_from
            .iter()
            .map(|l| serde_json::json!({ "requestedReviewer": { "login": l } }))
            .collect();
        Item::Pr(
            serde_json::from_value(serde_json::json!({
                "number": n, "title": "t", "isDraft": false, "updatedAt": "", "headRefName": "b",
                "reviewDecision": null, "author": { "login": author },
                "assignees": { "nodes": [] }, "reviewRequests": { "nodes": reviewers },
                "commits": { "nodes": [] },
            }))
            .unwrap(),
        )
    }

    #[test]
    fn grouping() {
        // Newest first: repo 0 (involved), repo 1 (review asked), repo 2 (pinned).
        let entries = vec![
            Entry {
                repo: 0,
                item: pr(1, "x", &[]),
            },
            Entry {
                repo: 1,
                item: pr(2, "x", &[]),
            },
            Entry {
                repo: 1,
                item: pr(3, "x", &["me"]),
            },
            Entry {
                repo: 2,
                item: pr(4, "me", &[]),
            },
        ];
        let pinned = |r: usize| (r == 2).then_some(0);
        let g = group(&entries, |_| true, "me", pinned);
        assert_eq!(g, vec![(2, vec![3]), (1, vec![2, 1]), (0, vec![0])]);
        let g = group(&entries, |e| e.repo != 1, "me", pinned);
        assert_eq!(g, vec![(2, vec![3]), (0, vec![0])]);
    }
}

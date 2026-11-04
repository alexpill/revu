//! Home dashboard and one tab per configured repo (PRs / issues).

use super::issue::IssueScreen;
use super::ui::{self, DIM, SELECTED};
use super::{Action, Ctx, Msg, Screen};
use crate::gh::{self, model::*};
use anyhow::Result;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState, Tabs};

pub const HELP: &[(&str, &str)] = &[
    ("1-9 H/L Tab", "switch tab (1 = home)"),
    ("j/k g/G", "move"),
    ("Enter", "open PR / issue"),
    ("p / i", "repo tab: pull requests / issues"),
    ("f", "cycle PR filter (all, review requested, assigned, mine, drafts)"),
    ("/", "filter PRs / search issues (GitHub syntax)"),
    ("o", "open in browser"),
    ("Ctrl-r", "refresh"),
    ("q", "quit"),
];

const DEFAULT_ISSUE_QUERY: &str = "is:open sort:updated-desc";

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
    const ALL: [Filter; 5] = [Filter::All, Filter::Review, Filter::Assigned, Filter::Mine, Filter::Draft];

    fn label(self) -> &'static str {
        match self {
            Filter::All => "all",
            Filter::Review => "review requested",
            Filter::Assigned => "assigned",
            Filter::Mine => "mine",
            Filter::Draft => "drafts",
        }
    }

    fn keep(self, pr: &PrSummary, me: &str) -> bool {
        match self {
            Filter::All => true,
            Filter::Review => pr.review_requested(me),
            Filter::Assigned => pr.assigned_to(me),
            Filter::Mine => pr.author_is(me),
            Filter::Draft => pr.is_draft,
        }
    }
}

struct RepoTab {
    issues_view: bool,
    filter: Filter,
    text: String,
    prs: TableState,
    issue_query: String,
    issues: Option<Result<Vec<IssueSummary>, String>>,
    issue_sel: TableState,
}

pub struct MainScreen {
    /// 0 = home, i + 1 = repo i
    tab: usize,
    /// Per repo, `None` while loading.
    prs: Vec<Option<Result<RepoPrs, String>>>,
    home: TableState,
    repos: Vec<RepoTab>,
    /// Text being typed after `/`.
    input: Option<String>,
}

impl MainScreen {
    pub fn new(ctx: &mut Ctx) -> Self {
        let repos = (0..ctx.cfg.repos.len())
            .map(|_| RepoTab {
                issues_view: false,
                filter: Filter::All,
                text: String::new(),
                prs: TableState::default().with_selected(0),
                issue_query: DEFAULT_ISSUE_QUERY.into(),
                issues: None,
                issue_sel: TableState::default().with_selected(0),
            })
            .collect();
        let prs = vec![None; ctx.cfg.repos.len()];
        let mut s = Self { tab: 0, prs, home: TableState::default().with_selected(0), repos, input: None };
        s.refresh(ctx);
        s
    }

    fn refresh(&mut self, ctx: &mut Ctx) {
        for i in 0..ctx.cfg.repos.len() {
            let (o, n) = ctx.cfg.repos[i].owner_name();
            let (o, n) = (o.to_string(), n.to_string());
            ctx.spawn(move || Msg::RepoPrs(i, gh::repo_prs(&o, &n)));
        }
        if let Some(i) = self.repo_idx() {
            if self.repos[i].issues.is_some() {
                self.load_issues(i, ctx);
            }
        }
    }

    fn load_issues(&mut self, i: usize, ctx: &mut Ctx) {
        let (repo, q) = (ctx.repo(i).to_string(), self.repos[i].issue_query.clone());
        ctx.spawn(move || Msg::Issues(i, gh::issues(&repo, &q)));
    }

    pub fn on_prs(&mut self, i: usize, r: Result<RepoPrs>, ctx: &mut Ctx) {
        if let Ok(p) = &r {
            ctx.viewer = p.viewer.clone();
        }
        self.prs[i] = Some(r.map_err(|e| e.to_string()));
    }

    pub fn on_issues(&mut self, i: usize, r: Result<Vec<IssueSummary>>, _ctx: &mut Ctx) {
        self.repos[i].issues = Some(r.map_err(|e| e.to_string()));
    }

    fn repo_idx(&self) -> Option<usize> {
        self.tab.checked_sub(1)
    }

    fn loaded(&self, i: usize) -> Option<&RepoPrs> {
        self.prs[i].as_ref()?.as_ref().ok()
    }

    /// PRs that involve the viewer across all repos, most recently updated first.
    fn for_you<'a>(&'a self, me: &str) -> Vec<(usize, &'a PrSummary, &'static str)> {
        let mut out: Vec<_> = (0..self.prs.len())
            .filter_map(|i| Some((i, self.loaded(i)?)))
            .flat_map(|(i, r)| r.nodes.iter().map(move |pr| (i, pr)))
            .filter_map(|(i, pr)| {
                let why = if pr.review_requested(me) {
                    "review"
                } else if pr.assigned_to(me) {
                    "assigned"
                } else if pr.author_is(me) {
                    "author"
                } else {
                    return None;
                };
                Some((i, pr, why))
            })
            .collect();
        out.sort_by(|a, b| b.1.updated_at.cmp(&a.1.updated_at));
        out
    }

    fn visible_prs(&self, i: usize, me: &str) -> Vec<&PrSummary> {
        let Some(prs) = self.loaded(i) else { return Vec::new() };
        let t = &self.repos[i];
        let needle = t.text.to_lowercase();
        prs.nodes
            .iter()
            .filter(|pr| t.filter.keep(pr, me))
            .filter(|pr| {
                needle.is_empty()
                    || format!("#{} {} {} {}", pr.number, pr.title, ui::login(&pr.author), pr.head_ref_name)
                        .to_lowercase()
                        .contains(&needle)
            })
            .collect()
    }

    /// The table cursor currently in use and its row count.
    fn cursor(&mut self, me: &str) -> (&mut TableState, usize) {
        match self.repo_idx() {
            None => {
                let n = self.for_you(me).len();
                (&mut self.home, n)
            }
            Some(i) if self.repos[i].issues_view => {
                let n = self.repos[i].issues.as_ref().and_then(|r| r.as_ref().ok()).map_or(0, Vec::len);
                (&mut self.repos[i].issue_sel, n)
            }
            Some(i) => {
                let n = self.visible_prs(i, me).len();
                (&mut self.repos[i].prs, n)
            }
        }
    }

    /// The PR or issue under the cursor, as (is_pr, key).
    fn selected(&mut self, me: &str) -> Option<(bool, super::Key)> {
        let (state, _) = self.cursor(me);
        let sel = state.selected()?;
        match self.repo_idx() {
            None => self.for_you(me).get(sel).map(|(i, pr, _)| (true, (*i, pr.number))),
            Some(i) if self.repos[i].issues_view => {
                let issues = self.repos[i].issues.as_ref()?.as_ref().ok()?;
                issues.get(sel).map(|is| (false, (i, is.number)))
            }
            Some(i) => self.visible_prs(i, me).get(sel).map(|pr| (true, (i, pr.number))),
        }
    }

    pub fn on_key(&mut self, k: KeyEvent, ctx: &mut Ctx) -> Action {
        if let Some(input) = &mut self.input {
            match k.code {
                KeyCode::Esc => self.input = None,
                KeyCode::Enter => {
                    let text = self.input.take().unwrap_or_default();
                    if let Some(i) = self.repo_idx() {
                        if self.repos[i].issues_view {
                            self.repos[i].issue_query = if text.trim().is_empty() { DEFAULT_ISSUE_QUERY.into() } else { text };
                            self.repos[i].issues = None;
                            self.repos[i].issue_sel.select(Some(0));
                            self.load_issues(i, ctx);
                        }
                    }
                }
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Char(c) => input.push(c),
                _ => {}
            }
            // PR filtering is live while typing.
            if let (Some(i), Some(text)) = (self.repo_idx(), &self.input) {
                if !self.repos[i].issues_view {
                    self.repos[i].text = text.clone();
                    self.repos[i].prs.select(Some(0));
                }
            }
            return Action::None;
        }

        let me = ctx.viewer.clone();
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let tabs = self.repos.len() + 1;
        match k.code {
            KeyCode::Char('q') => return Action::Quit,
            KeyCode::Esc => {
                if let Some(i) = self.repo_idx() {
                    self.repos[i].text.clear();
                }
            }
            KeyCode::Char('?') => return Action::Help,
            KeyCode::Char('r') if ctrl => self.refresh(ctx),
            KeyCode::Tab | KeyCode::Char('L') => self.tab = (self.tab + 1) % tabs,
            KeyCode::BackTab | KeyCode::Char('H') => self.tab = (self.tab + tabs - 1) % tabs,
            KeyCode::Char(c @ '1'..='9') => {
                let t = c as usize - '1' as usize;
                if t < tabs {
                    self.tab = t;
                }
            }
            KeyCode::Char('j') | KeyCode::Down => {
                let (s, n) = self.cursor(&me);
                s.select(Some((s.selected().unwrap_or(0) + 1).min(n.saturating_sub(1))));
            }
            KeyCode::Char('k') | KeyCode::Up => {
                let (s, _) = self.cursor(&me);
                s.select(Some(s.selected().unwrap_or(0).saturating_sub(1)));
            }
            KeyCode::Char('g') => self.cursor(&me).0.select(Some(0)),
            KeyCode::Char('G') => {
                let (s, n) = self.cursor(&me);
                s.select(Some(n.saturating_sub(1)));
            }
            KeyCode::Char('p') => {
                if let Some(i) = self.repo_idx() {
                    self.repos[i].issues_view = false;
                }
            }
            KeyCode::Char('i') => {
                if let Some(i) = self.repo_idx() {
                    self.repos[i].issues_view = true;
                    if self.repos[i].issues.is_none() {
                        self.load_issues(i, ctx);
                    }
                }
            }
            KeyCode::Char('f') => {
                if let Some(i) = self.repo_idx() {
                    let t = &mut self.repos[i];
                    let pos = Filter::ALL.iter().position(|f| *f == t.filter).unwrap_or(0);
                    t.filter = Filter::ALL[(pos + 1) % Filter::ALL.len()];
                    t.prs.select(Some(0));
                }
            }
            KeyCode::Char('/') => {
                if let Some(i) = self.repo_idx() {
                    let t = &self.repos[i];
                    self.input = Some(if t.issues_view { t.issue_query.clone() } else { t.text.clone() });
                }
            }
            KeyCode::Char('o') => {
                if let Some((_, key)) = self.selected(&me) {
                    ctx.browse(key);
                }
            }
            KeyCode::Enter => match self.selected(&me) {
                Some((false, key)) => return Action::Push(Screen::Issue(IssueScreen::open(key, ctx))),
                Some((true, _key)) => ctx.info("PR view not implemented yet"),
                None => {}
            },
            _ => {}
        }
        Action::None
    }

    pub fn render(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let [tabs_area, body] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(area);
        let mut titles = vec![Line::from(" 1 home ")];
        for (i, r) in ctx.cfg.repos.iter().enumerate() {
            let count = self.loaded(i).map(|p| format!(" {}", p.total_count)).unwrap_or_default();
            titles.push(Line::from(vec![format!(" {} {}", i + 2, r.name).into(), count.dark_gray(), " ".into()]));
        }
        f.render_widget(
            Tabs::new(titles).select(self.tab).highlight_style(Style::new().fg(ui::ACCENT).bold().reversed()).divider(""),
            tabs_area,
        );

        if ctx.cfg.repos.is_empty() {
            let msg = format!(" no repos configured: add [[repos]] to {}", crate::config::config_path().display());
            return f.render_widget(Paragraph::new(msg).yellow(), body);
        }
        match self.repo_idx() {
            None => self.render_home(f, body, ctx),
            Some(i) if self.repos[i].issues_view => self.render_issues(f, body, i),
            Some(i) => match &self.prs[i] {
                None => f.render_widget(Paragraph::new(" loading pull requests…").dark_gray(), body),
                Some(Err(e)) => f.render_widget(Paragraph::new(format!(" {e}")).red(), body),
                Some(Ok(_)) => self.render_prs(f, body, i, ctx),
            },
        }
    }

    fn render_home(&mut self, f: &mut Frame, area: Rect, ctx: &Ctx) {
        let me = &ctx.viewer;
        let summary: Vec<Row> = ctx
            .cfg
            .repos
            .iter()
            .enumerate()
            .map(|(i, cfg)| {
                let r = match &self.prs[i] {
                    None => return Row::new([format!("{} {}", i + 2, cfg.name), "…".into()]).style(DIM),
                    Some(Err(e)) => return Row::new([format!("{} {}", i + 2, cfg.name), e.clone()]).red(),
                    Some(Ok(r)) => r,
                };
                let count = |f: Filter| r.nodes.iter().filter(|p| f.keep(p, me)).count();
                let failing = r.nodes.iter().filter(|p| matches!(p.ci(), Some("FAILURE" | "ERROR"))).count();
                let n = |v: usize| if v == 0 { Cell::from("·").style(DIM) } else { Cell::from(v.to_string()) };
                Row::new(vec![
                    Cell::from(format!("{} {}", i + 2, cfg.name)),
                    Cell::from(r.total_count.to_string()),
                    n(count(Filter::Review)).yellow(),
                    n(count(Filter::Assigned)),
                    n(count(Filter::Mine)),
                    n(count(Filter::Draft)),
                    n(failing).red(),
                ])
            })
            .collect();
        let h = summary.len() as u16 + 3;
        let [top, bottom] = Layout::vertical([Constraint::Length(h), Constraint::Fill(1)]).areas(area);
        let header = Row::new(["repository", "open", "review", "assigned", "mine", "drafts", "failing"]).style(DIM);
        let widths = [Constraint::Fill(1), Constraint::Length(5), Constraint::Length(7), Constraint::Length(9), Constraint::Length(5), Constraint::Length(7), Constraint::Length(8)];
        f.render_widget(Table::new(summary, widths).header(header).block(ui::block("repositories", false)), top);

        let items = self.for_you(me);
        let rows: Vec<Row> = items
            .iter()
            .map(|(i, pr, why)| {
                let why = match *why {
                    "review" => why.yellow(),
                    _ => why.dark_gray(),
                };
                Row::new(vec![
                    Cell::from(ui::ci(pr.ci())),
                    Cell::from(ctx.repo(*i).rsplit('/').next().unwrap_or_default().to_string()).style(DIM),
                    Cell::from(format!("#{}", pr.number)).style(DIM),
                    Cell::from(title(pr)),
                    Cell::from(why),
                    Cell::from(ui::ago(&pr.updated_at)).style(DIM),
                ])
            })
            .collect();
        let widths = [Constraint::Length(1), Constraint::Length(18), Constraint::Length(6), Constraint::Fill(1), Constraint::Length(8), Constraint::Length(4)];
        let table = Table::new(rows, widths).row_highlight_style(SELECTED).block(ui::block(format!("for you ({})", items.len()), true));
        f.render_stateful_widget(table, bottom, &mut self.home);
    }

    fn render_prs(&mut self, f: &mut Frame, area: Rect, i: usize, ctx: &Ctx) {
        let me = &ctx.viewer;
        let total = self.loaded(i).map_or(0, |p| p.total_count);
        let prs = self.visible_prs(i, me);
        let rows: Vec<Row> = prs
            .iter()
            .map(|pr| {
                let you = if pr.review_requested(me) { "◆".yellow() } else { " ".into() };
                Row::new(vec![
                    Cell::from(ui::ci(pr.ci())),
                    Cell::from(format!("#{}", pr.number)).style(DIM),
                    Cell::from(Line::from(vec![you, " ".into()])),
                    Cell::from(title(pr)),
                    Cell::from(ui::login(&pr.author)).style(DIM),
                    Cell::from(ui::decision(pr.review_decision.as_deref())),
                    Cell::from(Line::from(vec![format!("+{}", pr.additions).green(), format!(" -{}", pr.deletions).red()])),
                    Cell::from(ui::ago(&pr.updated_at)).style(DIM),
                ])
            })
            .collect();
        let t = &self.repos[i];
        let mut title = format!("pull requests · {} · {}/{}", t.filter.label(), prs.len(), total);
        if !t.text.is_empty() {
            title += &format!(" · /{}", t.text);
        }
        let widths = [
            Constraint::Length(1),
            Constraint::Length(6),
            Constraint::Length(2),
            Constraint::Fill(1),
            Constraint::Length(16),
            Constraint::Length(8),
            Constraint::Length(13),
            Constraint::Length(4),
        ];
        let table = Table::new(rows, widths).row_highlight_style(SELECTED).block(ui::block(title, true));
        let area = self.render_input(f, area);
        f.render_stateful_widget(table, area, &mut self.repos[i].prs);
    }

    fn render_issues(&mut self, f: &mut Frame, area: Rect, i: usize) {
        let t = &mut self.repos[i];
        let title = format!("issues · {}", t.issue_query);
        let rows: Vec<Row> = match &t.issues {
            None => vec![Row::new(["loading…"]).style(DIM)],
            Some(Err(e)) => vec![Row::new([e.clone()]).red()],
            Some(Ok(issues)) => issues
                .iter()
                .map(|is| {
                    let state = if is.state == "OPEN" { "●".green() } else { "●".magenta() };
                    let labels: Vec<Span> = is.labels.nodes.iter().map(|l| format!(" {}", l.name).dark_gray()).collect();
                    Row::new(vec![
                        Cell::from(state),
                        Cell::from(format!("#{}", is.number)).style(DIM),
                        Cell::from(Line::from([vec![Span::from(is.title.clone())], labels].concat())),
                        Cell::from(ui::login(&is.author)).style(DIM),
                        Cell::from(format!("💬{}", is.comments.total_count)).style(DIM),
                        Cell::from(ui::ago(&is.updated_at)).style(DIM),
                    ])
                })
                .collect(),
        };
        let widths = [Constraint::Length(1), Constraint::Length(6), Constraint::Fill(1), Constraint::Length(16), Constraint::Length(5), Constraint::Length(4)];
        let table = Table::new(rows, widths).row_highlight_style(SELECTED).block(ui::block(title, true));
        let area = self.render_input(f, area);
        f.render_stateful_widget(table, area, &mut self.repos[i].issue_sel);
    }

    /// Draws the `/` prompt (if active) at the bottom; returns the remaining area.
    fn render_input(&self, f: &mut Frame, area: Rect) -> Rect {
        let Some(input) = &self.input else { return area };
        let [rest, line] = Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area);
        f.render_widget(Paragraph::new(Line::from(vec!["/".cyan(), input.clone().into(), "█".dark_gray()])), line);
        rest
    }
}

fn title(pr: &PrSummary) -> Line<'static> {
    if pr.is_draft {
        Line::from(vec!["draft ".dark_gray().italic(), pr.title.clone().dark_gray()])
    } else {
        Line::from(pr.title.clone())
    }
}

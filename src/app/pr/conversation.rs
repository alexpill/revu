//! The PR "front page": header, description, timeline and a sidebar with
//! reviewers, assignees, labels and checks.

use crate::app::ui::{self, DIM};
use crate::gh::model::{PrDetail, Thread};
use crate::highlight::Highlighter;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use std::collections::BTreeMap;

pub fn state_badge(d: &PrDetail) -> Span<'static> {
    match (d.state.as_str(), d.is_draft) {
        ("MERGED", _) => " merged ".on_magenta().black(),
        ("CLOSED", _) => " closed ".on_red().black(),
        (_, true) => " draft ".on_dark_gray().white(),
        _ => " open ".on_green().black(),
    }
}

/// Returns the clamped scroll offset.
pub fn render(f: &mut Frame, area: Rect, d: &PrDetail, hl: &Highlighter, scroll: u16) -> u16 {
    let [main, side] = Layout::horizontal([Constraint::Fill(1), Constraint::Length(38)]).areas(area);

    let mut lines = vec![
        Line::from(vec![
            ui::login(&d.author).bold(),
            " wants to merge into ".dark_gray(),
            d.base_ref_name.clone().cyan(),
            " from ".dark_gray(),
            d.head_ref_name.clone().cyan(),
        ]),
        Line::from(vec![
            format!("+{}", d.additions).green(),
            format!(" -{}", d.deletions).red(),
            format!(" · {} files · opened {} ago", d.changed_files, ui::ago(&d.created_at)).dark_gray(),
        ]),
        Line::default(),
    ];
    let body = if d.body.trim().is_empty() { "_No description provided._" } else { &d.body };
    lines.extend(ui::comment(&d.author, &d.created_at, "".into(), body, hl));
    lines.extend(timeline(d, hl));

    let scroll = scroll.min(lines.len().saturating_sub(1) as u16);
    let p = Paragraph::new(lines).wrap(Wrap { trim: false }).scroll((scroll, 0)).block(ui::block("conversation", true));
    f.render_widget(p, main);
    f.render_widget(Paragraph::new(sidebar(d)).wrap(Wrap { trim: true }).block(ui::block("details", false)), side);
    scroll
}

/// Comments, reviews and review threads, oldest first.
fn timeline(d: &PrDetail, hl: &Highlighter) -> Vec<Line<'static>> {
    let mut items: Vec<(&str, Vec<Line<'static>>)> = Vec::new();
    for c in &d.comments.nodes {
        items.push((&c.created_at, ui::comment(&c.author, &c.created_at, "".into(), &c.body, hl)));
    }
    for r in &d.reviews.nodes {
        let Some(at) = &r.submitted_at else { continue };
        if r.state == "COMMENTED" && r.body.is_empty() {
            continue; // just a container for inline threads, shown below
        }
        items.push((at, ui::comment(&r.author, at, review_state(&r.state), &r.body, hl)));
    }
    for t in &d.review_threads.nodes {
        if let Some(first) = t.comments.nodes.first() {
            items.push((&first.created_at, thread(t, hl)));
        }
    }
    items.sort_by(|a, b| a.0.cmp(b.0));
    items.into_iter().flat_map(|(_, l)| l).collect()
}

pub fn review_state(s: &str) -> Span<'static> {
    match s {
        "APPROVED" => "approved".green(),
        "CHANGES_REQUESTED" => "requested changes".red(),
        "DISMISSED" => "dismissed".dark_gray(),
        "PENDING" => "pending".yellow(),
        _ => "reviewed".dark_gray(),
    }
}

/// A review thread; resolved ones are folded to their header.
pub fn thread(t: &Thread, hl: &Highlighter) -> Vec<Line<'static>> {
    let line = t.line.or(t.original_line).map_or(String::new(), |l| format!(":{l}"));
    let mut header = vec!["💬 ".into(), format!("{}{line}", t.path).cyan()];
    if t.is_resolved {
        header.push(" resolved".green());
    }
    if t.is_outdated {
        header.push(" outdated".dark_gray());
    }
    let mut lines = vec![Line::from(header)];
    if !t.is_resolved {
        for c in &t.comments.nodes {
            let badge = if c.state == "PENDING" { "pending".yellow() } else { "".into() };
            lines.extend(ui::comment(&c.author, &c.created_at, badge, &c.body, hl));
        }
    } else {
        lines.push(Line::default());
    }
    lines
}

fn sidebar(d: &PrDetail) -> Vec<Line<'static>> {
    let section = |s: &str| Line::from(s.to_string()).bold().fg(ui::ACCENT);
    let mut l = vec![section("Reviewers")];
    // Latest meaningful state per reviewer.
    let mut states: BTreeMap<String, &str> = BTreeMap::new();
    for r in &d.reviews.nodes {
        if r.state == "PENDING" {
            continue;
        }
        let e = states.entry(ui::login(&r.author)).or_insert(&r.state);
        if r.state != "COMMENTED" {
            *e = &r.state;
        }
    }
    for rr in &d.review_requests.nodes {
        if let Some(a) = &rr.requested_reviewer {
            states.insert(a.login.clone(), "REQUESTED");
        }
    }
    if states.is_empty() {
        l.push(Line::from("none").style(DIM));
    }
    for (who, s) in states {
        let badge = if s == "REQUESTED" { "awaiting".yellow() } else { review_state(s) };
        l.push(Line::from(vec![Span::from(format!("{who} ")), badge]));
    }
    if let Some(dec) = &d.review_decision {
        l.push(Line::from(vec!["→ ".dark_gray(), ui::decision(Some(dec))]));
    }

    l.push(Line::default());
    l.push(section("Assignees"));
    let who: Vec<_> = d.assignees.nodes.iter().map(|a| a.login.clone()).collect();
    l.push(if who.is_empty() { Line::from("none").style(DIM) } else { Line::from(who.join(", ")) });

    if !d.labels.nodes.is_empty() {
        l.push(Line::default());
        l.push(section("Labels"));
        let labels: Vec<Span> = d.labels.nodes.iter().flat_map(|x| [format!(" {} ", x.name).on_dark_gray(), " ".into()]).collect();
        l.push(Line::from(labels));
    }

    l.push(Line::default());
    let merge = match d.mergeable.as_str() {
        "MERGEABLE" => "no conflicts".green(),
        "CONFLICTING" => "conflicts".red(),
        _ => "checking…".dark_gray(),
    };
    l.push(Line::from(vec!["Merge ".bold().fg(ui::ACCENT), merge]));

    if let Some(r) = d.rollup() {
        l.push(Line::default());
        l.push(Line::from(vec!["Checks ".bold().fg(ui::ACCENT), ui::ci(Some(&r.state))]));
        let mut checks: Vec<_> = r.contexts.iter().flat_map(|c| &c.nodes).collect();
        // Failures first: that's what you look for.
        checks.sort_by_key(|c| !matches!(ui::ci(Some(c.result())).content.as_ref(), "✗"));
        for c in checks {
            l.push(Line::from(vec![ui::ci(Some(c.result())), format!(" {}", c.label()).into()]));
        }
    }
    l
}

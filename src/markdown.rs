//! Just enough Markdown for PR descriptions and comments: headings, lists,
//! quotes, task boxes, fenced code (highlighted), inline `code` and **bold**.

use crate::highlight::Highlighter;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

pub fn render(src: &str, hl: &Highlighter) -> Vec<Line<'static>> {
    let src = strip_html_comments(src);
    let mut out = Vec::new();
    let mut fence: Option<(String, Vec<&str>)> = None;
    for raw in src.lines() {
        let trimmed = raw.trim_start();
        if trimmed.starts_with("```") {
            match fence.take() {
                Some((lang, code)) => {
                    for spans in hl.token(&lang, code) {
                        out.push(Line::from([vec![Span::raw("  ")], spans].concat()));
                    }
                }
                None => fence = Some((trimmed[3..].trim().to_string(), Vec::new())),
            }
            continue;
        }
        if let Some((_, code)) = &mut fence {
            code.push(raw);
            continue;
        }
        out.push(block_line(raw));
    }
    // Unterminated fence: show it raw rather than dropping it.
    if let Some((_, code)) = fence {
        out.extend(code.into_iter().map(|c| Line::from(c.to_string())));
    }
    out
}

fn block_line(raw: &str) -> Line<'static> {
    let t = raw.trim_start();
    let indent = " ".repeat(raw.len() - t.len());
    if let Some(level) = t.split_once(' ').filter(|(h, _)| !h.is_empty() && h.chars().all(|c| c == '#')) {
        return Line::from(Span::styled(level.1.to_string(), Style::new().fg(Color::Cyan).bold()));
    }
    if let Some(rest) = t.strip_prefix('>') {
        return Line::from(Span::styled(format!("▎ {}", rest.trim_start()), Style::new().dark_gray().italic()));
    }
    let (bullet, rest) = if let Some(r) = t.strip_prefix("- [ ] ").or_else(|| t.strip_prefix("* [ ] ")) {
        ("☐ ", r)
    } else if let Some(r) = ["- [x] ", "- [X] ", "* [x] "].iter().find_map(|p| t.strip_prefix(p)) {
        ("☑ ", r)
    } else if let Some(r) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
        ("• ", r)
    } else {
        ("", t)
    };
    let mut spans = vec![Span::raw(indent), Span::styled(bullet, Style::new().fg(Color::Yellow))];
    spans.extend(inline(rest));
    Line::from(spans)
}

/// `code` and **bold** spans.
fn inline(s: &str) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut rest = s;
    while !rest.is_empty() {
        let next = [rest.find('`'), rest.find("**")].into_iter().flatten().min();
        let Some(i) = next else { break };
        let (marker, style) = if rest[i..].starts_with('`') {
            ("`", Style::new().fg(Color::LightRed))
        } else {
            ("**", Style::new().add_modifier(Modifier::BOLD))
        };
        let after = &rest[i + marker.len()..];
        let Some(end) = after.find(marker) else { break };
        spans.push(Span::raw(rest[..i].to_string()));
        spans.push(Span::styled(after[..end].to_string(), style));
        rest = &after[end + marker.len()..];
    }
    spans.push(Span::raw(rest.to_string()));
    spans
}

fn strip_html_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("<!--") {
        out += &rest[..start];
        match rest[start..].find("-->") {
            Some(end) => rest = &rest[start + end + 3..],
            None => return out,
        }
    }
    out + rest
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(l: &Line) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn renders_blocks() {
        let hl = Highlighter::new(None);
        let lines = render("## Title\n<!-- hidden -->\n- [x] done `x` **b**\n```rust\nfn a() {}\n```", &hl);
        let t: Vec<_> = lines.iter().map(text).collect();
        assert_eq!(t, vec!["Title", "", "☑ done x b", "  fn a() {}"]);
    }
}

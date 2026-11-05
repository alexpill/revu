//! Just enough Markdown for PR descriptions and comments: headings, lists,
//! quotes, task boxes, fenced code (highlighted), inline `code`, **bold** and
//! links. HTML tags (bots love `<picture>`/`<details>`) are dropped, keeping
//! image alt text.

use crate::highlight::Highlighter;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

pub fn render(src: &str, hl: &Highlighter) -> Vec<Line<'static>> {
    let src = strip_html_comments(src);
    let mut out = Vec::new();
    let mut fence: Option<(String, Vec<&str>)> = None;
    for raw in src.lines() {
        let trimmed = raw.trim_start();
        if let Some(info) = trimmed.strip_prefix("```") {
            match fence.take() {
                Some((lang, code)) => {
                    for spans in hl.token(&lang, code) {
                        out.push(Line::from([vec![Span::raw("  ")], spans].concat()));
                    }
                }
                None => fence = Some((info.trim().to_string(), Vec::new())),
            }
            continue;
        }
        if let Some((_, code)) = &mut fence {
            code.push(raw);
            continue;
        }
        for part in strip_tags(raw).split('\n') {
            let blank = part.trim().is_empty();
            // Collapse runs of blank lines (often left behind by stripped HTML).
            if blank && out.last().is_none_or(|l: &Line| l.width() == 0) {
                continue;
            }
            out.push(if blank { Line::default() } else { block_line(part) });
        }
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
    if let Some(level) = t
        .split_once(' ')
        .filter(|(h, _)| !h.is_empty() && h.chars().all(|c| c == '#'))
    {
        return Line::from(Span::styled(level.1.to_string(), Style::new().fg(Color::Cyan).bold()));
    }
    if t.len() >= 3 && (t.chars().all(|c| c == '-') || t.chars().all(|c| c == '*')) {
        return Line::from(Span::styled("─".repeat(40), Style::new().fg(Color::DarkGray)));
    }
    if let Some(rest) = t.strip_prefix('>') {
        return Line::from(Span::styled(
            format!("▎ {}", rest.trim_start()),
            Style::new().dark_gray().italic(),
        ));
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

/// `code`, **bold** and [links](url) / ![images](url).
fn inline(s: &str) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut rest = s;
    loop {
        let next = [rest.find('`'), rest.find("**"), rest.find('[')]
            .into_iter()
            .flatten()
            .min();
        let Some(i) = next else { break };
        let (before, text, style, consumed) = if rest[i..].starts_with('[') {
            // [text](url) keeps the text, ![alt](url) becomes ⟨alt⟩.
            let Some((label, after)) = rest[i + 1..].split_once("](") else {
                break;
            };
            let Some(end) = after.find(')') else { break };
            let consumed = i + 1 + label.len() + 2 + end + 1;
            match rest[..i].strip_suffix('!') {
                Some(before) => (before, format!("⟨{label}⟩"), Style::new(), consumed),
                None => (
                    &rest[..i],
                    label.to_string(),
                    Style::new().add_modifier(Modifier::UNDERLINED),
                    consumed,
                ),
            }
        } else {
            let (marker, style) = if rest[i..].starts_with('`') {
                ("`", Style::new().fg(Color::LightRed))
            } else {
                ("**", Style::new().add_modifier(Modifier::BOLD))
            };
            let after = &rest[i + marker.len()..];
            let Some(end) = after.find(marker) else { break };
            (
                &rest[..i],
                after[..end].to_string(),
                style,
                i + marker.len() + end + marker.len(),
            )
        };
        spans.push(Span::raw(before.to_string()));
        spans.push(Span::styled(text, style));
        rest = &rest[consumed..];
    }
    spans.push(Span::raw(rest.to_string()));
    spans
}

const HTML_TAGS: &[&str] = &[
    "a",
    "b",
    "blockquote",
    "br",
    "code",
    "dd",
    "details",
    "div",
    "dl",
    "dt",
    "em",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hr",
    "i",
    "img",
    "kbd",
    "li",
    "ol",
    "p",
    "picture",
    "pre",
    "source",
    "span",
    "strong",
    "sub",
    "summary",
    "sup",
    "table",
    "tbody",
    "td",
    "th",
    "thead",
    "tr",
    "ul",
];

/// Drops known HTML tags outside `code` spans; `<img alt>` becomes ⟨alt⟩, `<br>` a newline.
fn strip_tags(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut in_code = false;
    let mut rest = line;
    while let Some(c) = rest.chars().next() {
        if c == '`' {
            in_code = !in_code;
        }
        if c == '<'
            && !in_code
            && let Some(end) = rest.find('>')
        {
            let tag = &rest[1..end];
            let name = tag
                .trim_start_matches('/')
                .split(|c: char| c.is_whitespace() || c == '/')
                .next()
                .unwrap_or("");
            if HTML_TAGS.contains(&name.to_ascii_lowercase().as_str()) {
                match name {
                    "br" => out.push('\n'),
                    "img" => {
                        if let Some(alt) = tag.split("alt=\"").nth(1).and_then(|a| a.split('"').next()) {
                            out += &format!("⟨{alt}⟩");
                        }
                    }
                    _ => {}
                }
                rest = &rest[end + 1..];
                continue;
            }
        }
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
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
        let lines = render(
            "## Title\n<!-- hidden -->\n\n- [x] done `x` **b**\n```rust\nfn a() {}\n```",
            &hl,
        );
        let t: Vec<_> = lines.iter().map(text).collect();
        assert_eq!(t, vec!["Title", "", "☑ done x b", "  fn a() {}"]);
        let t: Vec<_> = render(
            "<picture><img alt=\"High\" src=x></picture> see [docs](http://x) `Vec<T>`<br>![logo](y)",
            &hl,
        )
        .iter()
        .map(text)
        .collect();
        assert_eq!(t, vec!["⟨High⟩ see docs Vec<T>", "⟨logo⟩"]);
    }
}

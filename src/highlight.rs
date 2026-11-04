//! Syntax highlighting to ratatui spans (syntect + bat's syntax/theme set).

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Theme};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use two_face::theme::{EmbeddedLazyThemeSet, EmbeddedThemeName};

pub type StyledLine = Vec<Span<'static>>;

pub struct Highlighter {
    syntaxes: SyntaxSet,
    theme: Theme,
}

impl Highlighter {
    /// `theme` is a bat theme name (e.g. "Nord", "Catppuccin Mocha"), case-insensitive.
    pub fn new(theme: Option<&str>) -> Self {
        let name = EmbeddedLazyThemeSet::theme_names()
            .iter()
            .copied()
            .find(|n| theme.is_some_and(|t| n.as_name().eq_ignore_ascii_case(t)))
            .unwrap_or(EmbeddedThemeName::OneHalfDark);
        Self { syntaxes: two_face::syntax::extra_newlines(), theme: two_face::theme::extra().get(name).clone() }
    }

    fn syntax_for_path(&self, path: &str) -> &SyntaxReference {
        let file = path.rsplit('/').next().unwrap_or(path);
        let ext = file.rsplit_once('.').map_or(file, |(_, e)| e);
        self.syntaxes
            .find_syntax_by_extension(ext)
            .or_else(|| self.syntaxes.find_syntax_by_extension(file))
            .unwrap_or_else(|| self.syntaxes.find_syntax_plain_text())
    }

    /// Highlights consecutive lines of the file at `path`.
    pub fn file<'a>(&self, path: &str, lines: impl IntoIterator<Item = &'a str>) -> Vec<StyledLine> {
        self.run(self.syntax_for_path(path), lines)
    }

    /// For fenced code blocks: `token` is the language after the ```.
    pub fn token<'a>(&self, token: &str, lines: impl IntoIterator<Item = &'a str>) -> Vec<StyledLine> {
        let syntax = self.syntaxes.find_syntax_by_token(token).unwrap_or_else(|| self.syntaxes.find_syntax_plain_text());
        self.run(syntax, lines)
    }

    fn run<'a>(&self, syntax: &SyntaxReference, lines: impl IntoIterator<Item = &'a str>) -> Vec<StyledLine> {
        let mut h = HighlightLines::new(syntax, &self.theme);
        lines
            .into_iter()
            .map(|line| {
                let with_nl = format!("{line}\n");
                match h.highlight_line(&with_nl, &self.syntaxes) {
                    Ok(parts) => parts
                        .into_iter()
                        .map(|(s, t)| Span::styled(t.trim_end_matches('\n').replace('\t', "    "), style(s)))
                        .collect(),
                    Err(_) => vec![Span::raw(line.replace('\t', "    "))],
                }
            })
            .collect()
    }
}

fn style(s: syntect::highlighting::Style) -> Style {
    let mut st = Style::default().fg(Color::Rgb(s.foreground.r, s.foreground.g, s.foreground.b));
    if s.font_style.contains(FontStyle::BOLD) {
        st = st.add_modifier(Modifier::BOLD);
    }
    if s.font_style.contains(FontStyle::ITALIC) {
        st = st.add_modifier(Modifier::ITALIC);
    }
    st
}

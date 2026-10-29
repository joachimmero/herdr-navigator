//! Syntax highlighting in the terminal's own palette, so it reads on a light or dark background.

use std::path::Path;
use std::sync::OnceLock;

use ratatui::style::{Color, Style};
use ratatui::text::Span;
use syntect::easy::HighlightLines;
use syntect::highlighting::Theme;
use syntect::parsing::{SyntaxReference, SyntaxSet};
use two_face::theme::EmbeddedThemeName;

fn syntaxes() -> &'static SyntaxSet {
    static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
    SYNTAXES.get_or_init(two_face::syntax::extra_newlines)
}

fn theme() -> &'static Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    THEME.get_or_init(|| two_face::theme::extra().get(EmbeddedThemeName::Ansi).clone())
}

/// `lines` of the file at `path` as styled spans, one list per line, plain where no grammar
/// knows the file.
pub fn highlight(path: &Path, lines: &[&str]) -> Vec<Vec<Span<'static>>> {
    let Some(syntax) = syntax_for(path, lines.first().copied().unwrap_or_default()) else {
        return lines.iter().map(|line| vec![Span::raw((*line).to_owned())]).collect();
    };
    let mut highlighter = HighlightLines::new(syntax, theme());
    lines
        .iter()
        .map(|line| {
            // The grammars are the newline-terminated set; each line is fed as its own row.
            let fed = format!("{line}\n");
            match highlighter.highlight_line(&fed, syntaxes()) {
                Ok(regions) => regions
                    .into_iter()
                    .map(|(style, text)| {
                        Span::styled(text.trim_end_matches('\n').to_owned(), ansi(style.foreground))
                    })
                    .filter(|span| !span.content.is_empty())
                    .collect(),
                // A grammar error degrades to plain text rather than hiding the line.
                Err(_) => vec![Span::raw((*line).to_owned())],
            }
        })
        .collect()
}

fn syntax_for(path: &Path, first_line: &str) -> Option<&'static SyntaxReference> {
    let syntaxes = syntaxes();
    let by_extension = path
        .extension()
        .or_else(|| path.file_name())
        .and_then(|name| syntaxes.find_syntax_by_extension(&name.to_string_lossy()));
    by_extension.or_else(|| syntaxes.find_syntax_by_first_line(first_line))
}

/// The ANSI theme's color encoding: alpha 0 carries a palette index in red, alpha 1 the
/// terminal's default foreground.
fn ansi(color: syntect::highlighting::Color) -> Style {
    match color.a {
        0 => Style::new().fg(Color::Indexed(color.r)),
        1 => Style::new(),
        _ => Style::new().fg(Color::Rgb(color.r, color.g, color.b)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(lines: &[Vec<Span>]) -> Vec<String> {
        lines.iter().map(|spans| spans.iter().map(|span| span.content.as_ref()).collect()).collect()
    }

    #[test]
    fn rust_splits_into_palette_colored_spans_that_keep_the_text() {
        let lines = highlight(Path::new("main.rs"), &["let x = 1;", "fn main() {}"]);
        assert_eq!(text(&lines), ["let x = 1;", "fn main() {}"]);
        assert!(lines[0].len() > 1);
        assert!(lines[0].iter().any(|span| matches!(span.style.fg, Some(Color::Indexed(_)))));
    }

    #[test]
    fn an_unknown_file_is_one_plain_span_per_line() {
        let lines = highlight(Path::new("notes.unknownext"), &["alpha", "beta"]);
        assert_eq!(lines, [vec![Span::raw("alpha")], vec![Span::raw("beta")]]);
    }

    #[test]
    fn a_shebang_picks_the_grammar_when_the_name_cannot() {
        let lines = highlight(Path::new("run"), &["#!/bin/bash", "echo hi"]);
        assert!(lines[1].len() > 1);
    }
}

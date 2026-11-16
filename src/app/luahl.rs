//! Syntax colouring for the built-in Lua editor. Colour strings like "#ff375f"
//! are drawn on a chip of that colour, so a palette reads at a glance.

use egui::text::{LayoutJob, TextFormat};
use egui::{Color32, FontId};

use crate::config::{Syntax, color};

const KEYWORDS: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if", "in",
    "local", "nil", "not", "or", "repeat", "return", "then", "true", "until", "while",
];

#[derive(Clone, Copy)]
enum Tok {
    Plain,
    Keyword,
    Field,
    Str,
    Num,
    Comment,
    Punct,
}

/// Split Lua source into (start, end, kind) byte ranges that cover all of it.
fn tokens(s: &str) -> Vec<(usize, usize, Tok)> {
    let b = s.as_bytes();
    let n = b.len();
    let mut out = Vec::new();
    let mut i = 0;
    // `]]`-closed long bracket starting at i (after `[[`)
    let long_end = |from: usize| s[from..].find("]]").map_or(n, |p| from + p + 2);
    while i < n {
        let c = b[i];
        let start = i;
        let kind = if c == b'-' && b.get(i + 1) == Some(&b'-') {
            i = if s[i + 2..].starts_with("[[") {
                long_end(i + 4)
            } else {
                s[i..].find('\n').map_or(n, |p| i + p)
            };
            Tok::Comment
        } else if c == b'"' || c == b'\'' {
            i += 1;
            while i < n && b[i] != c && b[i] != b'\n' {
                i += if b[i] == b'\\' { 2 } else { 1 };
            }
            i = (i + 1).min(n);
            Tok::Str
        } else if c == b'[' && b.get(i + 1) == Some(&b'[') {
            i = long_end(i + 2);
            Tok::Str
        } else if c.is_ascii_digit() || (c == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit))
        {
            while i < n && (b[i].is_ascii_alphanumeric() || b[i] == b'.' || b[i] == b'_') {
                i += 1;
            }
            Tok::Num
        } else if c.is_ascii_alphabetic() || c == b'_' {
            while i < n && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            let word = &s[start..i];
            // `name =` (but not `name ==`) is a table field / assignment
            let rest = s[i..].trim_start_matches([' ', '\t']);
            if KEYWORDS.contains(&word) {
                Tok::Keyword
            } else if rest.starts_with('=') && !rest.starts_with("==") {
                Tok::Field
            } else {
                Tok::Plain
            }
        } else if b"{}[]()=,;.".contains(&c) {
            i += 1;
            Tok::Punct
        } else {
            // whitespace and anything else, one UTF-8 character at a time
            i += s[i..].chars().next().map_or(1, char::len_utf8);
            Tok::Plain
        };
        out.push((start, i.min(n), kind));
    }
    out
}

/// Black or white, whichever reads better on `bg`.
fn contrast(bg: Color32) -> Color32 {
    let l = 0.299 * bg.r() as f32 + 0.587 * bg.g() as f32 + 0.114 * bg.b() as f32;
    if l > 140.0 {
        Color32::BLACK
    } else {
        Color32::WHITE
    }
}

pub fn highlight(src: &str, syn: &Syntax, font: FontId, fg: Color32) -> LayoutJob {
    let mut job = LayoutJob::default();
    for (a, b, kind) in tokens(src) {
        let text = &src[a..b];
        let mut fmt = TextFormat {
            font_id: font.clone(),
            color: fg,
            ..Default::default()
        };
        match kind {
            Tok::Plain => {}
            Tok::Keyword => fmt.color = syn.keyword,
            Tok::Field => fmt.color = syn.field,
            Tok::Num => fmt.color = syn.number,
            Tok::Comment => fmt.color = syn.comment,
            Tok::Punct => fmt.color = syn.punct,
            Tok::Str => {
                let inner = text.trim_matches(['"', '\'']);
                match color(inner).filter(|_| inner.starts_with('#')) {
                    // a colour: show it as a chip of that colour
                    Some(c) => {
                        let chip = c.to_opaque();
                        fmt.background = chip;
                        fmt.color = contrast(chip);
                    }
                    None => fmt.color = syn.string,
                }
            }
        }
        job.append(text, 0.0, fmt);
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(s: &str) -> Vec<(String, &'static str)> {
        tokens(s)
            .into_iter()
            .filter(|(a, b, _)| !s[*a..*b].trim().is_empty())
            .map(|(a, b, k)| {
                let k = match k {
                    Tok::Plain => "plain",
                    Tok::Keyword => "kw",
                    Tok::Field => "field",
                    Tok::Str => "str",
                    Tok::Num => "num",
                    Tok::Comment => "comment",
                    Tok::Punct => "punct",
                };
                (s[a..b].to_string(), k)
            })
            .collect()
    }

    #[test]
    fn tokens_cover_the_whole_text() {
        let src = "local x = { a = \"#ff0000\", b = 1.5 } -- hi\n--[[ block ]] return x == 2 é";
        let t = tokens(src);
        assert_eq!(t.first().unwrap().0, 0);
        assert_eq!(t.last().unwrap().1, src.len());
        assert!(t.windows(2).all(|w| w[0].1 == w[1].0));
    }

    #[test]
    fn classifies_lua() {
        let k = kinds("local col = { red = \"#ff375f\", n = 10 } -- note\nreturn col.red == x");
        assert!(k.contains(&("local".into(), "kw")));
        assert!(k.contains(&("col".into(), "field")));
        assert!(k.contains(&("red".into(), "field")));
        assert!(k.contains(&("\"#ff375f\"".into(), "str")));
        assert!(k.contains(&("10".into(), "num")));
        assert!(k.contains(&("-- note".into(), "comment")));
        assert!(
            k.contains(&("x".into(), "plain")),
            "== is not an assignment"
        );
    }
}

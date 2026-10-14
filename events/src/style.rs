//! Terminal colors, switched off for pipes, `NO_COLOR` and `--color never`.

use std::io::IsTerminal;

use clap::ValueEnum;
use owo_colors::{OwoColorize, Style};

use crate::record::Op;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ColorChoice {
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone, Copy)]
pub struct Painter {
    on: bool,
}

impl Painter {
    pub fn new(on: bool) -> Painter {
        Painter { on }
    }

    pub fn for_stream(choice: ColorChoice, stream: &impl IsTerminal) -> Painter {
        let on = match choice {
            ColorChoice::Always => true,
            ColorChoice::Never => false,
            ColorChoice::Auto => {
                stream.is_terminal()
                    && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
                    && std::env::var("TERM").map_or(true, |t| t != "dumb")
            }
        };
        Painter { on }
    }

    pub fn paint(&self, text: &str, style: Style) -> String {
        if self.on {
            text.style(style).to_string()
        } else {
            text.to_owned()
        }
    }
}

pub fn op_style(op: Op) -> Style {
    match op {
        Op::Insert => Style::new().green().bold(),
        Op::Update => Style::new().yellow().bold(),
        Op::Delete => Style::new().red().bold(),
        Op::Other => Style::new().magenta().bold(),
    }
}

/// Green under a second, yellow under five, red beyond.
pub fn lag_style(ns: Option<i64>) -> Style {
    match ns {
        None => Style::new().dimmed(),
        Some(ns) if ns < 1_000_000_000 => Style::new().green(),
        Some(ns) if ns < 5_000_000_000 => Style::new().yellow(),
        Some(_) => Style::new().red(),
    }
}

pub fn dim() -> Style {
    Style::new().dimmed()
}

pub fn table() -> Style {
    Style::new().cyan()
}

pub fn warn() -> Style {
    Style::new().yellow()
}

pub fn bold() -> Style {
    Style::new().bold()
}

pub fn ok() -> Style {
    Style::new().green()
}

/// `events … | head` should just stop, not panic or complain.
pub fn exit_on_broken_pipe(e: &std::io::Error) {
    if e.kind() == std::io::ErrorKind::BrokenPipe {
        std::process::exit(0);
    }
}

/// Prints a line to stdout, exiting quietly if the reader has gone away.
pub fn println_or_exit(s: &str) {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    if let Err(e) = writeln!(out, "{s}").and_then(|_| out.flush()) {
        exit_on_broken_pipe(&e);
    }
}

/// A line built from styled segments, truncated by visible width (not bytes, not ANSI codes).
#[derive(Debug, Default, Clone)]
pub struct Line {
    segs: Vec<(String, Option<Style>)>,
}

impl Line {
    pub fn new() -> Line {
        Line::default()
    }

    pub fn push(&mut self, text: impl Into<String>, style: Style) -> &mut Line {
        self.segs.push((text.into(), Some(style)));
        self
    }

    pub fn plain(&mut self, text: impl Into<String>) -> &mut Line {
        self.segs.push((text.into(), None));
        self
    }

    pub fn width(&self) -> usize {
        self.segs.iter().map(|(t, _)| t.chars().count()).sum()
    }

    /// Renders, cutting the line (with `…`) so it's at most `max` characters wide.
    pub fn render(&self, p: &Painter, max: Option<usize>) -> String {
        let mut out = String::new();
        let mut used = 0;
        for (text, style) in &self.segs {
            let w = text.chars().count();
            let (text, last) = match max {
                Some(m) if used + w > m => {
                    (crate::format::truncate(text, m.saturating_sub(used)), true)
                }
                _ => (text.clone(), false),
            };
            match style {
                Some(s) if !text.is_empty() => out += &p.paint(&text, *s),
                _ => out += &text,
            }
            used += w;
            if last {
                break;
            }
        }
        out
    }
}

/// `1234567` → `1,234,567`.
pub fn fmt_count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Events per second: one decimal below 100, thousands separators above.
pub fn fmt_rate(r: f64) -> String {
    if r < 99.95 {
        format!("{r:.1}")
    } else {
        fmt_count(r.round() as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_truncates_by_visible_width() {
        let p = Painter::new(true);
        let mut l = Line::new();
        l.push("INSERT", op_style(Op::Insert))
            .plain(" ")
            .push("comments", table())
            .plain(" \"héllo wörld\"");
        assert_eq!(l.width(), 29);
        let plain = Painter::new(false);
        assert_eq!(l.render(&plain, None), "INSERT comments \"héllo wörld\"");
        assert_eq!(l.render(&plain, Some(20)), "INSERT comments \"hé…");
        assert_eq!(l.render(&plain, Some(15)), "INSERT comments");
        assert_eq!(l.render(&plain, Some(14)), "INSERT commen…");
        // Colors don't count towards the width.
        let colored = l.render(&p, Some(20));
        assert!(colored.contains("\u{1b}["));
        assert!(colored.ends_with("\"hé…"));
    }

    #[test]
    fn counts_and_rates() {
        assert_eq!(fmt_count(0), "0");
        assert_eq!(fmt_count(999), "999");
        assert_eq!(fmt_count(1000), "1,000");
        assert_eq!(fmt_count(1_234_567), "1,234,567");
        assert_eq!(fmt_rate(3.25), "3.2");
        assert_eq!(fmt_rate(2137.4), "2,137");
    }
}

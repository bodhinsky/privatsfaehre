//! Terminal output helpers (ANSI highlighting, timestamps).

use std::io::IsTerminal;

pub fn color_supported() -> bool {
    std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

/// A highlighted byte range.
#[derive(Debug, Clone, Copy)]
pub struct Mark {
    pub start: usize,
    pub end: usize,
    pub block: bool,
    pub focus: bool,
}

impl Mark {
    pub fn new(start: usize, end: usize, block: bool) -> Self {
        Self { start, end, block, focus: false }
    }
}

const RESET: &str = "\x1b[0m";

/// Render `text` with marks. Overlapping marks are merged into the first.
pub fn highlight(text: &str, marks: &[Mark], color: bool) -> String {
    let mut marks: Vec<Mark> = marks.to_vec();
    marks.sort_by_key(|m| (m.start, std::cmp::Reverse(m.end)));
    let mut out = String::with_capacity(text.len() + marks.len() * 12);
    let mut pos = 0;
    for m in marks {
        if m.start < pos {
            continue;
        }
        out.push_str(&text[pos..m.start]);
        let span = &text[m.start..m.end];
        if color {
            let style = match (m.focus, m.block) {
                (true, _) => "\x1b[1;7m",
                (false, true) => "\x1b[1;37;41m",
                (false, false) => "\x1b[30;43m",
            };
            out.push_str(style);
            out.push_str(span);
            out.push_str(RESET);
        } else {
            let (l, r) = if m.focus {
                ("⟦", "⟧")
            } else if m.block {
                ("‼", "‼")
            } else {
                ("«", "»")
            };
            out.push_str(l);
            out.push_str(span);
            out.push_str(r);
        }
        pos = m.end;
    }
    out.push_str(&text[pos..]);
    out
}

pub use session::now_rfc3339;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_highlight() {
        let t = "Anna zahlt";
        assert_eq!(highlight(t, &[Mark::new(0, 4, false)], false), "«Anna» zahlt");
    }
}

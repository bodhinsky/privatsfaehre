//! Terminal output helpers (ANSI highlighting, timestamps).

use std::io::IsTerminal;
use std::time::{SystemTime, UNIX_EPOCH};

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

/// Current UTC time as RFC 3339 (second precision).
pub fn now_rfc3339() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    rfc3339(secs)
}

fn rfc3339(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Civil-from-days (H. Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_709_210_096), "2024-02-29T12:34:56Z");
    }

    #[test]
    fn plain_highlight() {
        let t = "Anna zahlt";
        assert_eq!(highlight(t, &[Mark::new(0, 4, false)], false), "«Anna» zahlt");
    }
}

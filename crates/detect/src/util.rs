//! Small text helpers shared by recognizers.

pub(crate) fn floor_boundary(s: &str, mut i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

pub(crate) fn ceil_boundary(s: &str, mut i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    while !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// Up to `radius` bytes of text before `start`.
pub(crate) fn before(s: &str, start: usize, radius: usize) -> &str {
    &s[floor_boundary(s, start.saturating_sub(radius))..start]
}

/// Up to `radius` bytes of text after `end`.
pub(crate) fn after(s: &str, end: usize, radius: usize) -> &str {
    &s[end..ceil_boundary(s, end.saturating_add(radius))]
}

/// `radius` bytes on each side of `[start, end)`.
pub(crate) fn around(s: &str, start: usize, end: usize, radius: usize) -> &str {
    &s[floor_boundary(s, start.saturating_sub(radius))..ceil_boundary(s, end.saturating_add(radius))]
}

/// The sentence containing `[start, end)` (split on `.!?` and newlines),
/// widened to at least `min_radius` bytes on each side.
pub(crate) fn sentence_window(s: &str, start: usize, end: usize, min_radius: usize) -> &str {
    let bytes = s.as_bytes();
    let mut a = start;
    while a > 0 && !matches!(bytes[a - 1], b'.' | b'!' | b'?' | b'\n') {
        a -= 1;
    }
    let mut b = end;
    while b < bytes.len() && !matches!(bytes[b], b'.' | b'!' | b'?' | b'\n') {
        b += 1;
    }
    let a = a.min(floor_boundary(s, start.saturating_sub(min_radius)));
    let b = b.max(ceil_boundary(s, end.saturating_add(min_radius)));
    &s[a..b]
}

/// Move `end` left past any trailing characters in `chars`.
pub(crate) fn trim_end(s: &str, start: usize, mut end: usize, chars: &[char]) -> usize {
    while end > start {
        let Some(c) = s[..end].chars().next_back() else { break };
        if chars.contains(&c) {
            end -= c.len_utf8();
        } else {
            break;
        }
    }
    end
}

pub(crate) fn is_capitalized(word: &str) -> bool {
    word.chars().next().is_some_and(char::is_uppercase)
}

pub(crate) fn digits(s: &str) -> Vec<u8> {
    s.bytes().filter(u8::is_ascii_digit).map(|b| b - b'0').collect()
}

/// Shorthand for building a lazily-compiled static regex.
macro_rules! lazy_regex {
    ($name:ident, $re:expr) => {
        static $name: std::sync::LazyLock<regex::Regex> =
            std::sync::LazyLock::new(|| regex::Regex::new($re).expect(concat!("invalid regex ", stringify!($name))));
    };
}
pub(crate) use lazy_regex;

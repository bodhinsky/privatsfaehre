//! Model-independent pre- and post-processing (unit-tested without a model).

use detect::ner::NerSpan;
use regex::Regex;
use std::ops::Range;
use std::sync::LazyLock;
use tokenizers::{InputSequence, Tokenizer};

/// A word with byte offsets into the input text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Word {
    pub start: usize,
    pub end: usize,
}

// GLiNER's WhitespaceTokenSplitter.
static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\w+(?:[-_]\w+)*|\S").expect("regex"));

pub fn split_words(text: &str) -> Vec<Word> {
    WORD.find_iter(text).map(|m| Word { start: m.start(), end: m.end() }).collect()
}

/// Tokenized model input for one window.
#[derive(Debug, PartialEq)]
pub struct Input {
    pub input_ids: Vec<i64>,
    pub attention_mask: Vec<i64>,
    /// For the first sub-token of text word `k`: `k + 1`; otherwise 0.
    pub words_mask: Vec<i64>,
}

pub fn build_input(
    tokenizer: &Tokenizer,
    words: &[Word],
    text: &str,
    labels: &[&str],
    ent: &str,
    sep: &str,
) -> Result<Input, String> {
    let mut seq: Vec<&str> = Vec::with_capacity(labels.len() * 2 + 1 + words.len());
    for l in labels {
        seq.push(ent);
        seq.push(l);
    }
    seq.push(sep);
    let prompt_len = seq.len();
    seq.extend(words.iter().map(|w| &text[w.start..w.end]));
    let enc = tokenizer.encode(InputSequence::from(seq), true).map_err(|e| e.to_string())?;
    let mut words_mask = Vec::with_capacity(enc.len());
    let mut prev: Option<u32> = None;
    for wid in enc.get_word_ids() {
        let v = match *wid {
            Some(w) if Some(w) != prev && (w as usize) >= prompt_len => (w as usize - prompt_len + 1) as i64,
            _ => 0,
        };
        words_mask.push(v);
        prev = *wid;
    }
    Ok(Input {
        input_ids: enc.get_ids().iter().map(|&i| i64::from(i)).collect(),
        attention_mask: enc.get_attention_mask().iter().map(|&m| i64::from(m)).collect(),
        words_mask,
    })
}

/// `span_idx` (flattened `[n * w, 2]`, inclusive end word) and `span_mask`.
pub fn spans(n_words: usize, max_width: usize) -> (Vec<i64>, Vec<bool>) {
    let mut idx = Vec::with_capacity(n_words * max_width * 2);
    let mut mask = Vec::with_capacity(n_words * max_width);
    for i in 0..n_words {
        for j in 0..max_width {
            idx.push(i as i64);
            idx.push((i + j) as i64);
            mask.push(i + j < n_words);
        }
    }
    (idx, mask)
}

/// A scored span in word indices (inclusive).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scored {
    pub first: usize,
    pub last: usize,
    pub label: usize,
    pub score: f32,
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

/// Spans above `threshold` from `logits[1, rows, max_width, labels]`, flat
/// (non-overlapping) by descending score.
pub fn decode(
    logits: &[f32],
    rows: usize,
    max_width: usize,
    labels: usize,
    n_words: usize,
    threshold: f32,
) -> Vec<Scored> {
    let mut out = Vec::new();
    for i in 0..rows.min(n_words) {
        for j in 0..max_width {
            if i + j >= n_words {
                break;
            }
            for c in 0..labels {
                let Some(&x) = logits.get(((i * max_width) + j) * labels + c) else { continue };
                let score = sigmoid(x);
                if score > threshold {
                    out.push(Scored { first: i, last: i + j, label: c, score });
                }
            }
        }
    }
    greedy(out)
}

fn greedy(mut spans: Vec<Scored>) -> Vec<Scored> {
    spans.sort_by(|a, b| b.score.total_cmp(&a.score).then((a.first, a.last).cmp(&(b.first, b.last))));
    let mut kept: Vec<Scored> = Vec::new();
    for s in spans {
        if kept.iter().all(|k| s.last < k.first || k.last < s.first) {
            kept.push(s);
        }
    }
    kept.sort_by_key(|s| s.first);
    kept
}

/// Word windows of `size` overlapping by `overlap`, covering `0..n`.
pub fn windows(n: usize, size: usize, overlap: usize) -> Vec<Range<usize>> {
    let size = size.max(overlap + 1);
    let mut out = Vec::new();
    let mut start = 0;
    loop {
        let end = (start + size).min(n);
        out.push(start..end);
        if end == n {
            return out;
        }
        start = end - overlap;
    }
}

/// Merge predictions from overlapping windows: highest score wins on overlap.
pub fn flat(mut spans: Vec<NerSpan>) -> Vec<NerSpan> {
    spans.sort_by(|a, b| b.score.total_cmp(&a.score).then((a.start, a.end).cmp(&(b.start, b.end))));
    let mut kept: Vec<NerSpan> = Vec::new();
    for s in spans {
        if kept.iter().all(|k| s.end <= k.start || k.end <= s.start) {
            kept.push(s);
        }
    }
    kept.sort_by_key(|s| s.start);
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts<'a>(t: &'a str, w: &[Word]) -> Vec<&'a str> {
        w.iter().map(|w| &t[w.start..w.end]).collect()
    }

    #[test]
    fn words_like_gliner() {
        let t = "Frau Müller-Lüdenscheidt (52) wohnt in Köln, Tel. 0171/2345678.";
        let w = split_words(t);
        assert_eq!(
            texts(t, &w),
            [
                "Frau",
                "Müller-Lüdenscheidt",
                "(",
                "52",
                ")",
                "wohnt",
                "in",
                "Köln",
                ",",
                "Tel",
                ".",
                "0171",
                "/",
                "2345678",
                "."
            ]
        );
    }

    #[test]
    fn span_grid() {
        let (idx, mask) = spans(3, 2);
        assert_eq!(idx, [0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3]);
        assert_eq!(mask, [true, true, true, true, true, false]);
    }

    #[test]
    fn decode_is_flat_and_thresholded() {
        // 3 words, width 2, 2 labels. logits index = ((i * w) + j) * labels + c.
        let mut l = vec![-10.0f32; 3 * 2 * 2];
        let at = |i: usize, j: usize, c: usize| (i * 2 + j) * 2 + c;
        l[at(0, 1, 0)] = 3.0; // words 0..=1 label 0, score .95
        l[at(1, 0, 1)] = 1.0; // word 1 label 1, score .73, overlaps -> dropped
        l[at(2, 0, 1)] = 0.5; // word 2 label 1, score .62
        l[at(2, 1, 0)] = 9.0; // 2..=3 is out of range -> ignored
        let s = decode(&l, 3, 2, 2, 3, 0.5);
        assert_eq!(s.len(), 2, "{s:?}");
        assert_eq!((s[0].first, s[0].last, s[0].label), (0, 1, 0));
        assert_eq!((s[1].first, s[1].last, s[1].label), (2, 2, 1));
    }

    #[test]
    fn windows_overlap_and_cover() {
        assert_eq!(windows(5, 10, 2), vec![Range { start: 0, end: 5 }]);
        assert_eq!(windows(10, 4, 1), [0..4, 3..7, 6..10]);
        assert_eq!(windows(0, 4, 1), vec![Range { start: 0, end: 0 }]);
    }

    #[test]
    fn merge_windows() {
        let s = |start, end, score| NerSpan { start, end, label: "person".into(), score };
        let m = flat(vec![s(0, 10, 0.6), s(5, 12, 0.9), s(20, 25, 0.4), s(5, 12, 0.8)]);
        assert_eq!(m, [s(5, 12, 0.9), s(20, 25, 0.4)]);
    }
}

//! Text normalization for the risk lexicon (spec 7.3, "Matching").
//!
//! Page text and lexicon phrases go through the same function, so a phrase never needs one entry
//! per spelling. Two forms are kept for every word:
//!
//! * `acc`: lowercase, tone placement unified (`hoá` and `hóa` are the same), diacritics kept.
//! * `plain`: the same with every combining mark removed and `đ` mapped to `d`.
//!
//! Which form is compared is decided per text (a control name, a dialog text): a text that has
//! Vietnamese diacritics is compared with `acc`, a text without any is compared with `plain`.
//! Punctuation is a hard boundary (a new [`Segment`]), except a hyphen, underscore or apostrophe
//! between two letters or digits, so `thanh-toán` is `thanh toan` and `Sim số - Thẻ cào` is two
//! segments.

use unicode_normalization::UnicodeNormalization;

/// One word in both forms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Word {
    pub acc: String,
    pub plain: String,
}

/// A run of words with no punctuation between them.
pub type Segment = Vec<Word>;

/// A normalized text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Norm {
    pub segments: Vec<Segment>,
    /// The text has at least one Vietnamese diacritic (or `đ`).
    pub has_diacritics: bool,
}

impl Norm {
    /// Number of words over all segments.
    pub fn word_count(&self) -> usize {
        self.segments.iter().map(Vec::len).sum()
    }
    pub fn is_empty(&self) -> bool {
        self.segments.iter().all(Vec::is_empty)
    }
    /// Words joined with single spaces in the form the text is compared in, for display in tests.
    pub fn joined(&self, plain: bool) -> String {
        self.segments
            .iter()
            .flatten()
            .map(|w| {
                if plain {
                    w.plain.as_str()
                } else {
                    w.acc.as_str()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[derive(Clone)]
struct Cluster {
    base: char,
    marks: Vec<char>,
}

const TONE_MARKS: [char; 5] = ['\u{300}', '\u{301}', '\u{303}', '\u{309}', '\u{323}'];
/// Marks that occur in Vietnamese: the tones plus circumflex, breve and horn.
const VI_MARKS: [char; 8] = [
    '\u{300}', '\u{301}', '\u{302}', '\u{303}', '\u{306}', '\u{309}', '\u{31b}', '\u{323}',
];

fn clusters(text: &str) -> Vec<Cluster> {
    let mut out: Vec<Cluster> = Vec::new();
    for ch in text.to_lowercase().nfd() {
        if unicode_normalization::char::is_combining_mark(ch) {
            if let Some(last) = out.last_mut() {
                last.marks.push(ch);
                continue;
            }
        }
        out.push(Cluster {
            base: ch,
            marks: Vec::new(),
        });
    }
    out
}

fn tone_of(c: &Cluster) -> Option<char> {
    c.marks.iter().copied().find(|m| TONE_MARKS.contains(m))
}

/// Puts the tone of `oa`, `oe` and `uy` on the first vowel, so both spelling styles agree.
/// `qu` + `y` keeps its tone: there the `u` is part of the consonant.
fn unify_tone_placement(cs: &mut [Cluster]) {
    for i in 1..cs.len() {
        let after_q = i >= 2 && cs[i - 2].base == 'q';
        let (head, tail) = cs.split_at_mut(i);
        let (first, second) = (&mut head[i - 1], &mut tail[0]);
        let pair = matches!(
            (first.base, second.base),
            ('o', 'a') | ('o', 'e') | ('u', 'y')
        );
        if !pair || !first.marks.is_empty() {
            continue;
        }
        let Some(tone) = tone_of(second) else {
            continue;
        };
        if second.marks.len() != 1 {
            continue;
        }
        if first.base == 'u' && after_q {
            continue;
        }
        second.marks.clear();
        first.marks.push(tone);
    }
}

fn compose(c: &Cluster) -> String {
    std::iter::once(c.base)
        .chain(c.marks.iter().copied())
        .nfc()
        .collect()
}

fn plain_of(c: &Cluster) -> String {
    match c.base {
        'đ' => "d".to_owned(),
        b => b.to_string(),
    }
}

fn is_word_char(c: &Cluster) -> bool {
    c.base.is_alphanumeric()
}

/// Normalizes a text for matching.
pub fn normalize(text: &str) -> Norm {
    let mut cs = clusters(text);
    unify_tone_placement(&mut cs);
    let mut norm = Norm::default();
    let mut segment: Segment = Vec::new();
    let (mut acc, mut plain) = (String::new(), String::new());
    let flush_word = |acc: &mut String, plain: &mut String, segment: &mut Segment| {
        if !acc.is_empty() {
            segment.push(Word {
                acc: std::mem::take(acc),
                plain: std::mem::take(plain),
            });
        }
    };
    let flush_segment = |segment: &mut Segment, norm: &mut Norm| {
        if !segment.is_empty() {
            norm.segments.push(std::mem::take(segment));
        }
    };
    for (i, c) in cs.iter().enumerate() {
        if is_word_char(c) {
            if c.base == 'đ' || c.marks.iter().any(|m| VI_MARKS.contains(m)) {
                norm.has_diacritics = true;
            }
            acc.push_str(&compose(c));
            plain.push_str(&plain_of(c));
        } else if c.base.is_whitespace() {
            flush_word(&mut acc, &mut plain, &mut segment);
        } else {
            let joins =
                i > 0 && i + 1 < cs.len() && is_word_char(&cs[i - 1]) && is_word_char(&cs[i + 1]);
            match c.base {
                '-' | '_' | '\u{2010}' | '\u{2011}' if joins => {
                    flush_word(&mut acc, &mut plain, &mut segment);
                }
                '\'' | '\u{2019}' if joins => {}
                _ => {
                    flush_word(&mut acc, &mut plain, &mut segment);
                    flush_segment(&mut segment, &mut norm);
                }
            }
        }
    }
    flush_word(&mut acc, &mut plain, &mut segment);
    flush_segment(&mut segment, &mut norm);
    norm
}

/// The share of words that carry a Vietnamese diacritic, for the page-language fallback.
pub fn diacritic_share(text: &str) -> f32 {
    let mut total = 0usize;
    let mut marked = 0usize;
    for word in normalize(text).segments.iter().flatten() {
        total += 1;
        if word.acc != word.plain {
            marked += 1;
        }
    }
    if total == 0 {
        0.0
    } else {
        marked as f32 / total as f32
    }
}

/// Whether a page is Vietnamese: the `lang` attribute when there is one, else the share of words
/// with diacritics in a sample of the page text.
pub fn page_is_vietnamese(lang: Option<&str>, sample: &str) -> bool {
    match lang.map(str::trim).filter(|l| !l.is_empty()) {
        Some(l) => l.to_lowercase().starts_with("vi"),
        None => diacritic_share(sample) >= 0.2,
    }
}

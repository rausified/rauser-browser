//! Title templates (§9.1): parsing, the structural checks, and anchored matching.
//!
//! `P` is matched forward from the start of the tab title and `Q` backward
//! from its end. Captures take the longest run of their alphabet and skips
//! stop at the nearest occurrence of their bounding literal, found with a
//! Knuth-Morris-Pratt search, so no step is ever retried and each skip is
//! linear in the text it scans (§9.2, §10.2). Failure tables are built once,
//! at parse time.

use crate::ctype::{BaseType, CType, parse_bounds};
use crate::error::ErrorCode;
use crate::path::is_name;

pub(crate) const MAX_TITLE_CHARS: usize = 512;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Part {
    Literal(Vec<char>),
    Capture {
        name: String,
        base: BaseType,
        /// Bounds as written, if any; validation requires them to equal the
        /// path capture's effective bounds (SQ26).
        bounds: Option<(usize, usize)>,
    },
    Skip,
    Title,
}

/// A parsed title template. `ctypes` is filled during validation with the
/// same-named path or query capture's ctype, which canonicalizes the value.
#[derive(Clone, Debug)]
pub(crate) struct TitleTemplate {
    pub(crate) parts: Vec<Part>,
    /// Index of the `{title}` part.
    pub(crate) title_at: usize,
    pub(crate) ctypes: Vec<Option<CType>>,
    /// The character offset of each part in the template text.
    pub(crate) offsets: Vec<usize>,
    /// The KMP failure table of each literal, empty for other parts: over the
    /// literal in `P`, over the reversed literal in `Q`.
    fails: Vec<Vec<usize>>,
}

impl TitleTemplate {
    /// Parses one entry and applies `title_syntax` and
    /// `title_unbounded_capture`, with the character offset of the offending
    /// part. Later checks need the rules; see `document.rs`.
    pub(crate) fn parse(text: &str) -> Result<TitleTemplate, (ErrorCode, usize)> {
        let syntax = |at: usize| Err((ErrorCode::TitleSyntax, at));
        let chars: Vec<char> = text.chars().collect();
        let mut parts: Vec<Part> = Vec::new();
        let mut offsets = Vec::new();
        let mut i = 0;
        while i < chars.len() {
            match chars[i] {
                '}' => return syntax(i),
                '{' => {
                    let Some(len) = chars[i + 1..].iter().position(|&c| c == '}') else {
                        return syntax(i);
                    };
                    let content: String = chars[i + 1..i + 1 + len].iter().collect();
                    let part = match content.as_str() {
                        "title" => Part::Title,
                        "~" => Part::Skip,
                        _ => parse_title_capture(&content).ok_or((ErrorCode::TitleSyntax, i))?,
                    };
                    parts.push(part);
                    offsets.push(i);
                    i += len + 2;
                }
                c => {
                    if let Some(Part::Literal(lit)) = parts.last_mut() {
                        lit.push(c);
                    } else {
                        parts.push(Part::Literal(vec![c]));
                        offsets.push(i);
                    }
                    i += 1;
                }
            }
        }
        // A missing `{title}` concerns the whole entry; a second one is
        // reported where it starts.
        let mut titles = parts.iter().enumerate().filter(|(_, p)| **p == Part::Title);
        let title_at = match (titles.next(), titles.next()) {
            (Some((at, _)), None) => at,
            (Some(_), Some((second, _))) => return syntax(offsets[second]),
            (None, _) => return syntax(0),
        };
        if let Some(k) = (1..parts.len()).find(|&k| {
            !matches!(parts[k - 1], Part::Literal(_)) && !matches!(parts[k], Part::Literal(_))
        }) {
            return Err((ErrorCode::TitleUnboundedCapture, offsets[k]));
        }
        let fails = parts
            .iter()
            .enumerate()
            .map(|(i, p)| match p {
                Part::Literal(lit) if i < title_at => failure(lit.len(), |k| lit[k]),
                Part::Literal(lit) => failure(lit.len(), |k| lit[lit.len() - 1 - k]),
                _ => Vec::new(),
            })
            .collect();
        let ctypes = vec![None; parts.len()];
        Ok(TitleTemplate {
            parts,
            title_at,
            ctypes,
            offsets,
            fails,
        })
    }

    /// Named captures with their part index.
    pub(crate) fn captures(
        &self,
    ) -> impl Iterator<Item = (usize, &str, BaseType, Option<(usize, usize)>)> {
        self.parts.iter().enumerate().filter_map(|(i, p)| match p {
            Part::Capture { name, base, bounds } => Some((i, name.as_str(), *base, *bounds)),
            _ => None,
        })
    }

    /// `title_capture_swallows`: in `P`, the literal after a capture must start
    /// outside the capture's alphabet; in `Q`, the literal before it must end
    /// outside it. Returns the first capture that swallows its literal.
    pub(crate) fn swallows(&self) -> Option<usize> {
        (0..self.parts.len()).find(|&i| {
            let p = &self.parts[i];
            let Part::Capture { base, .. } = p else {
                return false;
            };
            let bound = if i < self.title_at {
                match self.parts.get(i + 1) {
                    Some(Part::Literal(l)) => l.first(),
                    _ => None,
                }
            } else {
                match i.checked_sub(1).and_then(|j| self.parts.get(j)) {
                    Some(Part::Literal(l)) => l.last(),
                    _ => None,
                }
            };
            bound.is_some_and(|&c| base.in_alphabet(c))
        })
    }

    /// Matches a prepared title. Returns the trimmed middle and the canonical
    /// value of each capture, or `None` when the template does not match.
    pub(crate) fn matches(&self, title: &[char]) -> Option<(String, Vec<(String, String)>)> {
        let mut caps = Vec::new();
        let prefix = &self.parts[..self.title_at];
        let mut pos = 0;
        for (i, part) in prefix.iter().enumerate() {
            match part {
                Part::Literal(lit) => {
                    if !title[pos..].starts_with(lit) {
                        return None;
                    }
                    pos += lit.len();
                }
                Part::Capture { name, base, .. } => {
                    let run = title[pos..]
                        .iter()
                        .take_while(|&&c| base.in_alphabet(c))
                        .count();
                    let raw: String = title[pos..pos + run].iter().collect();
                    caps.push((name.clone(), self.canonicalize(i, *base, &raw)?));
                    pos += run;
                }
                Part::Skip => {
                    let Some(Part::Literal(lit)) = prefix.get(i + 1) else {
                        return None;
                    };
                    pos += find_forward(&title[pos..], lit, &self.fails[i + 1])?;
                }
                Part::Title => unreachable!("prefix excludes the title part"),
            }
        }
        let suffix = &self.parts[self.title_at + 1..];
        let mut end = title.len();
        for (k, part) in suffix.iter().enumerate().rev() {
            let i = self.title_at + 1 + k;
            match part {
                Part::Literal(lit) => {
                    if !title[..end].ends_with(lit) {
                        return None;
                    }
                    end -= lit.len();
                }
                Part::Capture { name, base, .. } => {
                    let run = title[..end]
                        .iter()
                        .rev()
                        .take_while(|&&c| base.in_alphabet(c))
                        .count();
                    let raw: String = title[end - run..end].iter().collect();
                    caps.push((name.clone(), self.canonicalize(i, *base, &raw)?));
                    end -= run;
                }
                Part::Skip => {
                    let Some(Part::Literal(lit)) = k.checked_sub(1).and_then(|j| suffix.get(j))
                    else {
                        return None;
                    };
                    end = find_backward(&title[..end], lit, &self.fails[i - 1])?;
                }
                Part::Title => unreachable!("suffix excludes the title part"),
            }
        }
        if pos > end {
            return None;
        }
        let middle: String = title[pos..end].iter().collect();
        let trimmed = middle.trim_matches(char::is_whitespace);
        if trimmed.is_empty() {
            return None;
        }
        Some((trimmed.to_owned(), caps))
    }

    fn canonicalize(&self, i: usize, base: BaseType, raw: &str) -> Option<String> {
        match &self.ctypes[i] {
            Some(ctype) => ctype.canonical_whole(raw),
            None => base.canonical(base.default_bounds().unwrap_or((0, 0)), raw),
        }
    }
}

/// Parses `name:type_name[bounds]`. Title captures take no modifiers,
/// alternations, or `@tail`.
fn parse_title_capture(content: &str) -> Option<Part> {
    let (name, ty) = content.split_once(':')?;
    if !is_name(name) {
        return None;
    }
    let len = ty
        .bytes()
        .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
        .count();
    let base = BaseType::from_name(&ty[..len])?;
    let rest = &ty[len..];
    let bounds = if rest.is_empty() {
        None
    } else {
        let (bounds, after) = parse_bounds(rest)?;
        if !after.is_empty() {
            return None;
        }
        Some(bounds)
    };
    Some(Part::Capture {
        name: name.to_owned(),
        base,
        bounds,
    })
}

/// The offset of the first occurrence of `needle` in `hay`, given the
/// needle's failure table.
fn find_forward(hay: &[char], needle: &[char], fail: &[usize]) -> Option<usize> {
    kmp(hay.len(), needle.len(), |i| hay[i], |i| needle[i], fail)
}

/// The end offset of the last occurrence of `needle` in `hay`: the first
/// occurrence of the reversed needle in the reversed text, read back. `fail`
/// is the reversed needle's failure table.
fn find_backward(hay: &[char], needle: &[char], fail: &[usize]) -> Option<usize> {
    let (h, n) = (hay.len(), needle.len());
    kmp(h, n, |i| hay[h - 1 - i], |i| needle[n - 1 - i], fail).map(|start| h - start)
}

/// The KMP failure table of an indexed sequence of length `n`: `fail[i]` is
/// the length of the longest proper border of `needle[..=i]`.
fn failure(n: usize, needle: impl Fn(usize) -> char) -> Vec<usize> {
    let mut fail = vec![0usize; n];
    let mut k = 0;
    for i in 1..n {
        while k > 0 && needle(i) != needle(k) {
            k = fail[k - 1];
        }
        if needle(i) == needle(k) {
            k += 1;
        }
        fail[i] = k;
    }
    fail
}

/// Knuth-Morris-Pratt search over indexed sequences of lengths `h` and `n`:
/// the start of the first occurrence, in O(h) comparisons given the needle's
/// failure table. A needle longer than the text cannot occur.
fn kmp(
    h: usize,
    n: usize,
    hay: impl Fn(usize) -> char,
    needle: impl Fn(usize) -> char,
    fail: &[usize],
) -> Option<usize> {
    if n == 0 {
        return Some(0);
    }
    if n > h {
        return None;
    }
    let mut k = 0;
    for i in 0..h {
        while k > 0 && hay(i) != needle(k) {
            k = fail[k - 1];
        }
        if hay(i) == needle(k) {
            k += 1;
        }
        if k == n {
            return Some(i + 1 - n);
        }
    }
    None
}

/// Converts a tab title into the matching input: well-formed (the caller's
/// `&str` already is) and capped at 512 scalar values.
pub(crate) fn prepare_title(title: &str) -> Vec<char> {
    title.chars().take(MAX_TITLE_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(t: &str, title: &str) -> Option<String> {
        TitleTemplate::parse(t)
            .unwrap()
            .matches(&prepare_title(title))
            .map(|(m, _)| m)
    }

    #[test]
    fn skips_stop_at_the_nearest_delimiter() {
        assert_eq!(
            run(
                "{title} - {~} - Confluence",
                "A - B - Team Space - Confluence"
            )
            .as_deref(),
            Some("A - B")
        );
        assert_eq!(run("({~}) {title} - Mail", "(3) - Mail"), None);
        assert_eq!(run("{title} - Mail", "(3) - Mail").as_deref(), Some("(3)"));
    }

    #[test]
    fn structure_errors() {
        let code = |t: &str| TitleTemplate::parse(t).err().map(|e| e.0);
        assert_eq!(code("{title} - {title}"), Some(ErrorCode::TitleSyntax));
        assert_eq!(code("{~}{title}"), Some(ErrorCode::TitleUnboundedCapture));
        assert_eq!(code("{n:int(3)} {title}"), None);
        assert_eq!(code("{n:int|lower} {title}"), Some(ErrorCode::TitleSyntax));
    }

    #[test]
    fn structure_errors_point_at_the_offending_part() {
        let err = |t: &str| TitleTemplate::parse(t).err();
        assert_eq!(err("{title} - {title}"), Some((ErrorCode::TitleSyntax, 10)));
        assert_eq!(err("no title"), Some((ErrorCode::TitleSyntax, 0)));
        assert_eq!(
            err("a {~}{title}"),
            Some((ErrorCode::TitleUnboundedCapture, 5))
        );
        assert_eq!(
            err("{id:int}{~}{title}"),
            Some((ErrorCode::TitleUnboundedCapture, 8))
        );
    }

    fn find_forward(hay: &[char], needle: &[char]) -> Option<usize> {
        super::find_forward(hay, needle, &failure(needle.len(), |k| needle[k]))
    }

    fn find_backward(hay: &[char], needle: &[char]) -> Option<usize> {
        let n = needle.len();
        super::find_backward(hay, needle, &failure(n, |k| needle[n - 1 - k]))
    }

    #[test]
    fn failure_tables_are_built_at_parse_time() {
        let tpl = TitleTemplate::parse("ab{~}aab{title}aba{~}- X").unwrap();
        assert_eq!(tpl.fails[2], [0, 1, 0]);
        assert_eq!(tpl.fails[4], [0, 0, 1]);
        assert_eq!(tpl.fails[6], [0, 0, 0]);
        assert!(tpl.fails[1].is_empty());
    }

    #[test]
    fn searches_find_the_nearest_occurrence() {
        let c = |s: &str| s.chars().collect::<Vec<_>>();
        assert_eq!(find_forward(&c("a - b - c"), &c(" - ")), Some(1));
        assert_eq!(find_backward(&c("a - b - c"), &c(" - ")), Some(8));
        assert_eq!(find_forward(&c("aabaab"), &c("aab")), Some(0));
        assert_eq!(find_backward(&c("aabaab"), &c("aab")), Some(6));
        // A mismatch after a partial match restarts at the needle's border,
        // not at its start, so a self-overlapping needle is still found.
        assert_eq!(find_forward(&c("aaab"), &c("aab")), Some(1));
        assert_eq!(find_backward(&c("baaa"), &c("baa")), Some(3));
        assert_eq!(find_forward(&c("abababc"), &c("ababc")), Some(2));
        assert_eq!(find_forward(&c("abab"), &c("abc")), None);
        assert_eq!(find_backward(&c("ab"), &c("abc")), None);
        assert_eq!(find_forward(&c("ab"), &c("")), Some(0));
        assert_eq!(find_backward(&c("ab"), &c("")), Some(2));
    }

    #[test]
    fn searches_are_linear() {
        // A naive scan does about 10^10 comparisons here.
        let hay = vec!['a'; 200_000];
        let mut needle = vec!['a'; 100_000];
        needle.push('b');
        let start = std::time::Instant::now();
        assert_eq!(find_forward(&hay, &needle), None);
        needle.reverse();
        assert_eq!(find_backward(&hay, &needle), None);
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
    }
}

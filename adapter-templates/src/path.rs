//! Path templates (§4) and URL segment preparation (§5.1 steps 3-6).

use crate::ctype::{CType, check_literal, parse_alternation};
use crate::error::ErrorCode;

const MAX_TEMPLATE_CHARS: usize = 512;
const MAX_TEMPLATE_SEGMENTS: usize = 32;
const MAX_URL_SEGMENTS: usize = 64;
const MAX_ANONYMOUS_CHARS: usize = 256;
const MAX_NAME: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Segment {
    Literal(String),
    Alternation(Vec<String>),
    Capture {
        name: String,
        ctype: CType,
        optional: bool,
    },
    Anonymous,
    Rest,
}

/// A validated path template: required segments, then optionals, then an
/// optional rest.
#[derive(Clone, Debug)]
pub(crate) struct PathTemplate {
    pub(crate) segments: Vec<Segment>,
    /// Count of leading required segments.
    pub(crate) required: usize,
    /// Count of optional segments after them.
    pub(crate) optional: usize,
    pub(crate) rest: bool,
    /// The character offset of each segment in the template text.
    pub(crate) offsets: Vec<usize>,
}

/// `name = lower { lower | digit | "_" }`, at most 32.
pub(crate) fn is_name(s: &str) -> bool {
    let mut bytes = s.bytes();
    s.len() <= MAX_NAME
        && bytes.next().is_some_and(|b| b.is_ascii_lowercase())
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// Parses the inside of a `{name:ctype}` or `{name?:ctype}` capture. `{_}` and
/// `{*}` are handled by the caller.
pub(crate) fn parse_capture(content: &str) -> Result<(String, CType, bool), ErrorCode> {
    let (name, ctype) = content.split_once(':').ok_or(ErrorCode::PathSyntax)?;
    let (name, optional) = match name.strip_suffix('?') {
        Some(n) => (n, true),
        None => (name, false),
    };
    if !is_name(name) {
        return Err(ErrorCode::BadCaptureName);
    }
    if name == "host" || name == "title" {
        return Err(ErrorCode::ReservedCaptureName);
    }
    Ok((name.to_owned(), CType::parse(ctype)?, optional))
}

impl PathTemplate {
    /// Parses and structurally checks a path template. On error, returns the
    /// code and the character offset of the offending segment: for
    /// `template_too_long` the first character past the limit, and for
    /// `no_literal_segment`, which concerns the whole template, 0.
    pub(crate) fn parse(text: &str) -> Result<PathTemplate, (ErrorCode, usize)> {
        let Some(body) = text.strip_prefix('/') else {
            return Err((ErrorCode::PathSyntax, 0));
        };
        let mut segments = Vec::new();
        let mut offsets = Vec::new();
        if !body.is_empty() {
            let mut offset = 1;
            for piece in body.split('/') {
                let seg = parse_segment(piece).map_err(|e| (e, offset))?;
                segments.push(seg);
                offsets.push(offset);
                offset += piece.chars().count() + 1;
            }
        }
        if text.chars().count() > MAX_TEMPLATE_CHARS {
            return Err((ErrorCode::TemplateTooLong, MAX_TEMPLATE_CHARS));
        }
        if segments.len() > MAX_TEMPLATE_SEGMENTS {
            return Err((ErrorCode::TooManySegments, offsets[MAX_TEMPLATE_SEGMENTS]));
        }
        if let Some(i) = segments.iter().position(|s| *s == Segment::Rest)
            && i + 1 != segments.len()
        {
            return Err((ErrorCode::RestNotLast, offsets[i]));
        }
        let is_optional = |s: &Segment| matches!(s, Segment::Capture { optional: true, .. });
        let required = segments
            .iter()
            .take_while(|s| !is_optional(s) && **s != Segment::Rest)
            .count();
        let optional = segments[required..]
            .iter()
            .take_while(|s| is_optional(s))
            .count();
        let rest = segments.last() == Some(&Segment::Rest);
        // The rest is last, so the first segment out of place is a required
        // one after an optional.
        if required + optional + usize::from(rest) != segments.len() {
            return Err((ErrorCode::OptionalMisplaced, offsets[required + optional]));
        }
        if !segments
            .iter()
            .any(|s| matches!(s, Segment::Literal(_) | Segment::Alternation(_)))
        {
            return Err((ErrorCode::NoLiteralSegment, 0));
        }
        Ok(PathTemplate {
            segments,
            required,
            optional,
            rest,
            offsets,
        })
    }

    /// Each named capture in template order, with its segment's character
    /// offset.
    pub(crate) fn capture_offsets(&self) -> impl Iterator<Item = (&str, usize)> {
        self.segments
            .iter()
            .zip(&self.offsets)
            .filter_map(|(s, &at)| match s {
                Segment::Capture { name, .. } => Some((name.as_str(), at)),
                _ => None,
            })
    }

    /// Named captures in template order, with their optionality.
    pub(crate) fn captures(&self) -> impl Iterator<Item = (&str, &CType, bool)> {
        self.segments.iter().filter_map(|s| match s {
            Segment::Capture {
                name,
                ctype,
                optional,
            } => Some((name.as_str(), ctype, *optional)),
            _ => None,
        })
    }

    /// §4.3, verbatim. Each segment is examined at most once and no decision
    /// is revisited. Bound captures are appended to `out`.
    pub(crate) fn matches(
        &self,
        segs: &[String],
        fold: bool,
        out: &mut Vec<(String, String)>,
    ) -> bool {
        let n = segs.len();
        let (r, o) = (self.required, self.optional);
        if n < r || (!self.rest && n > r + o) {
            return false;
        }
        for (seg, value) in self.segments[..r].iter().zip(segs) {
            if !match_segment(seg, value, fold, out) {
                return false;
            }
        }
        for (j, seg) in self.segments[r..r + o].iter().enumerate() {
            if r + j >= n {
                break;
            }
            if !match_segment(seg, &segs[r + j], fold, out) {
                if self.rest {
                    break;
                }
                return false;
            }
        }
        true
    }
}

fn parse_segment(piece: &str) -> Result<Segment, ErrorCode> {
    if piece.starts_with('{') {
        let content = piece
            .strip_suffix('}')
            .filter(|_| piece.len() >= 2)
            .ok_or(ErrorCode::PathSyntax)?;
        let content = &content[1..];
        return Ok(match content {
            "_" => Segment::Anonymous,
            "*" => Segment::Rest,
            _ => {
                let (name, ctype, optional) = parse_capture(content)?;
                Segment::Capture {
                    name,
                    ctype,
                    optional,
                }
            }
        });
    }
    if piece.starts_with('(') {
        let (lits, rest) = parse_alternation(piece)?;
        if !rest.is_empty() {
            return Err(ErrorCode::PathSyntax);
        }
        return Ok(Segment::Alternation(lits));
    }
    check_literal(piece)?;
    Ok(Segment::Literal(piece.to_owned()))
}

fn match_segment(seg: &Segment, value: &str, fold: bool, out: &mut Vec<(String, String)>) -> bool {
    let eq = |lit: &str| {
        if fold {
            lit.eq_ignore_ascii_case(value)
        } else {
            lit == value
        }
    };
    match seg {
        Segment::Literal(lit) => eq(lit),
        Segment::Alternation(lits) => lits.iter().any(|l| eq(l)),
        Segment::Anonymous => !value.is_empty() && value.chars().count() <= MAX_ANONYMOUS_CHARS,
        Segment::Capture { name, ctype, .. } => match ctype.canonical_folded(value, fold) {
            Some(v) => {
                out.push((name.clone(), v));
                true
            }
            None => false,
        },
        Segment::Rest => true,
    }
}

/// §5.1 steps 3-6: turns a raw (still percent-encoded) pathname into decoded
/// segments, or `None` when the URL matches no rule.
pub fn prepare_segments(raw: &str) -> Option<Vec<String>> {
    let mut pieces: Vec<&str> = raw.split('/').collect();
    if pieces.first() == Some(&"") {
        pieces.remove(0);
    }
    if pieces.last() == Some(&"") {
        pieces.pop();
    }
    if pieces.len() > MAX_URL_SEGMENTS {
        return None;
    }
    let mut out = Vec::with_capacity(pieces.len());
    for piece in pieces {
        if piece.is_empty() {
            return None;
        }
        let decoded = percent_decode_strict(piece)?;
        if decoded == "."
            || decoded == ".."
            || decoded
                .chars()
                .any(|c| c == '/' || c <= '\u{1f}' || c == '\u{7f}')
        {
            return None;
        }
        out.push(decoded);
    }
    Some(out)
}

/// Decodes `%XX` escapes once and requires the result to be UTF-8. A `%` not
/// followed by two hex digits is invalid.
pub(crate) fn percent_decode_strict(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hi = hex_val(*bytes.get(i + 1)?)?;
            let lo = hex_val(*bytes.get(i + 2)?)?;
            out.push(hi << 4 | lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn hex_val(b: u8) -> Option<u8> {
    (b as char).to_digit(16).map(|d| d as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segs(p: &str) -> Option<Vec<String>> {
        prepare_segments(p)
    }

    #[test]
    fn segments_follow_section_5_1() {
        assert_eq!(segs("/"), Some(vec![]));
        assert_eq!(segs("/a/b/"), Some(vec!["a".into(), "b".into()]));
        assert_eq!(segs("/a//b"), None);
        assert_eq!(segs("/a%2Fb"), None);
        assert_eq!(segs("/%E2%82%AC"), Some(vec!["\u{20ac}".into()]));
        assert_eq!(segs("/%C0%AF"), None);
        assert_eq!(segs("/%zz"), None);
        assert_eq!(segs("/.%2e"), None);
        assert_eq!(segs("/%C2%80"), Some(vec!["\u{80}".into()]));
    }

    #[test]
    fn optionals_fall_back_to_the_rest() {
        let t = PathTemplate::parse("/d/{id:int}/{mode?:(edit|view)}/{*}").unwrap();
        let mut out = Vec::new();
        let s: Vec<String> = ["d", "7", "copy", "x"].map(String::from).to_vec();
        assert!(t.matches(&s, false, &mut out));
        assert_eq!(out, [("id".to_owned(), "7".to_owned())]);
    }

    #[test]
    fn structure_errors_point_at_the_offending_segment() {
        let at = |t: &str| PathTemplate::parse(t).err();
        assert_eq!(at("/x/{*}/{id:int}"), Some((ErrorCode::RestNotLast, 3)));
        assert_eq!(
            at("/x/{a?:int}/{b:int}"),
            Some((ErrorCode::OptionalMisplaced, 12))
        );
        assert_eq!(at("/{a:int}"), Some((ErrorCode::NoLiteralSegment, 0)));
        let many = format!("{}/x", "/ab".repeat(32));
        assert_eq!(at(&many), Some((ErrorCode::TooManySegments, 97)));
        let long = format!("/{}a", "a/".repeat(300));
        assert_eq!(at(&long), Some((ErrorCode::TemplateTooLong, 512)));
    }
}

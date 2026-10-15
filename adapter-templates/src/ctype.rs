//! Capture types (§7): parsing a ctype, checking a value, and canonicalizing it.
//!
//! Every check is one left-to-right pass over the value. Alphabets are ASCII,
//! so a value with any other character is rejected before length is counted.

use crate::error::ErrorCode;

pub(crate) const MAX_LITERAL: usize = 64;
pub(crate) const MAX_ALTERNATIVES: usize = 16;
const UUID_LEN: usize = 36;
/// The digit run of a `jira_key` is an `int` with its default bounds.
const MAX_INT_DIGITS: usize = 19;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BaseType {
    Int,
    Hex,
    Token,
    Name,
    JiraKey,
    Uuid,
}

impl BaseType {
    pub fn from_name(s: &str) -> Option<Self> {
        Some(match s {
            "int" => BaseType::Int,
            "hex" => BaseType::Hex,
            "token" => BaseType::Token,
            "name" => BaseType::Name,
            "jira_key" => BaseType::JiraKey,
            "uuid" => BaseType::Uuid,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            BaseType::Int => "int",
            BaseType::Hex => "hex",
            BaseType::Token => "token",
            BaseType::Name => "name",
            BaseType::JiraKey => "jira_key",
            BaseType::Uuid => "uuid",
        }
    }

    /// Default length bounds; for `jira_key` they bound the project part.
    /// `uuid` has a fixed shape and takes no bounds.
    pub(crate) fn default_bounds(self) -> Option<(usize, usize)> {
        match self {
            BaseType::Int => Some((1, MAX_INT_DIGITS)),
            BaseType::Hex => Some((1, 64)),
            BaseType::Token => Some((1, 128)),
            BaseType::Name => Some((1, 100)),
            BaseType::JiraKey => Some((1, 255)),
            BaseType::Uuid => None,
        }
    }

    /// The characters a title capture of this type takes. For `uuid` that is
    /// hex digits and `-`, without the braces, so a braced UUID in a title
    /// ends its run at the brace (SQ7).
    pub(crate) fn in_alphabet(self, c: char) -> bool {
        match self {
            BaseType::Int => c.is_ascii_digit(),
            BaseType::Hex => c.is_ascii_hexdigit(),
            BaseType::Token | BaseType::JiraKey => {
                c.is_ascii_alphanumeric() || c == '_' || c == '-'
            }
            BaseType::Name => c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'),
            BaseType::Uuid => c.is_ascii_hexdigit() || c == '-',
        }
    }

    /// Checks a whole value with the given bounds and returns its canonical
    /// form, before any case modifier.
    pub(crate) fn canonical(self, bounds: (usize, usize), v: &str) -> Option<String> {
        if !v.is_ascii() {
            return None;
        }
        let (min, max) = bounds;
        let in_len = |n: usize| (min..=max).contains(&n);
        match self {
            BaseType::Int => (is_int(v) && in_len(v.len())).then(|| v.to_owned()),
            BaseType::Hex => (in_len(v.len()) && v.bytes().all(|b| b.is_ascii_hexdigit()))
                .then(|| v.to_ascii_lowercase()),
            BaseType::Token => {
                (in_len(v.len()) && v.chars().all(|c| self.in_alphabet(c))).then(|| v.to_owned())
            }
            BaseType::Name => {
                (in_len(v.len()) && v != "." && v != ".." && v.chars().all(|c| self.in_alphabet(c)))
                    .then(|| v.to_owned())
            }
            BaseType::JiraKey => {
                let (project, number) = v.split_once('-')?;
                let mut bytes = project.bytes();
                let first_ok = bytes.next().is_some_and(|b| b.is_ascii_alphabetic());
                let rest_ok = bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_');
                (first_ok && rest_ok && in_len(project.len()) && is_int(number))
                    .then(|| v.to_ascii_uppercase())
            }
            BaseType::Uuid => {
                let inner = match v.strip_prefix('{') {
                    Some(rest) => rest.strip_suffix('}')?,
                    None => v,
                };
                (inner.len() == UUID_LEN
                    && inner.bytes().enumerate().all(|(i, b)| match i {
                        8 | 13 | 18 | 23 => b == b'-',
                        _ => b.is_ascii_hexdigit(),
                    }))
                .then(|| inner.to_ascii_lowercase())
            }
        }
    }

    /// The length range of a whole value, for the conservative overlap check.
    pub(crate) fn value_len(self, bounds: (usize, usize)) -> (usize, usize) {
        match self {
            // Project, `-`, then one to nineteen digits.
            BaseType::JiraKey => (bounds.0 + 2, bounds.1 + 1 + MAX_INT_DIGITS),
            BaseType::Uuid => (UUID_LEN, UUID_LEN + 2),
            _ => bounds,
        }
    }

    /// The alphabet as an ASCII bit set, for the overlap check. A `uuid`
    /// segment may also hold braces, but no other type's alphabet has them,
    /// so leaving them out cannot make two sets look disjoint.
    pub(crate) fn alphabet_mask(self) -> u128 {
        let mut mask = 0u128;
        for b in 0u8..128 {
            if self.in_alphabet(b as char) {
                mask |= 1 << b;
            }
        }
        mask
    }
}

/// ASCII digits, 1 to 19 of them, without a leading zero unless the value is `0`.
fn is_int(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= MAX_INT_DIGITS
        && v.bytes().all(|b| b.is_ascii_digit())
        && (v == "0" || !v.starts_with('0'))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaseMod {
    Lower,
    Upper,
}

/// A parsed ctype. Bounds are always stored in effective form, so `int` and
/// `int(1,19)` compare equal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CType {
    Typed {
        base: BaseType,
        /// Effective bounds; `(0, 0)` for `uuid`, which has none.
        bounds: (usize, usize),
        case: Option<CaseMod>,
        tail: bool,
    },
    /// The listed literals, in template order.
    Alternation(Vec<String>),
}

impl CType {
    /// Parses a ctype. The text runs to the end of the capture, so anything
    /// left over after the recognized parts is `path_syntax` (SQ-V12).
    pub fn parse(text: &str) -> Result<CType, ErrorCode> {
        if text.starts_with('(') {
            let (lits, rest) = parse_alternation(text)?;
            if !rest.is_empty() {
                return Err(ErrorCode::PathSyntax);
            }
            for lit in &lits {
                if lit == "." || lit == ".." || !lit.bytes().all(is_id_literal_byte) {
                    return Err(ErrorCode::BadCaptureLiteral);
                }
            }
            return Ok(CType::Alternation(lits));
        }
        let name_len = text
            .bytes()
            .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
            .count();
        if name_len == 0 {
            return Err(ErrorCode::PathSyntax);
        }
        let base = BaseType::from_name(&text[..name_len]).ok_or(ErrorCode::UnknownCaptureType)?;
        let mut rest = &text[name_len..];
        let mut bounds = base.default_bounds().unwrap_or((0, 0));
        if rest.starts_with('(') {
            let (parsed, after) = parse_bounds(rest).ok_or(ErrorCode::PathSyntax)?;
            let Some((dmin, dmax)) = base.default_bounds() else {
                return Err(ErrorCode::BadBounds);
            };
            if parsed.0 < dmin || parsed.0 > parsed.1 || parsed.1 > dmax {
                return Err(ErrorCode::BadBounds);
            }
            bounds = parsed;
            rest = after;
        }
        let mut case = None;
        for (text, m) in [("|lower", CaseMod::Lower), ("|upper", CaseMod::Upper)] {
            if let Some(after) = rest.strip_prefix(text) {
                if !matches!(base, BaseType::Token | BaseType::Name) {
                    return Err(ErrorCode::CaseModNotAllowed);
                }
                case = Some(m);
                rest = after;
                break;
            }
        }
        let mut tail = false;
        if let Some(after) = rest.strip_prefix("@tail") {
            if !matches!(base, BaseType::Int | BaseType::Hex) {
                return Err(ErrorCode::TailNotAllowed);
            }
            tail = true;
            rest = after;
        }
        if !rest.is_empty() {
            return Err(ErrorCode::PathSyntax);
        }
        Ok(CType::Typed {
            base,
            bounds,
            case,
            tail,
        })
    }

    pub fn is_alternation(&self) -> bool {
        matches!(self, CType::Alternation(_))
    }

    /// The canonical value of one decoded segment or query value, or `None`
    /// when it is not a valid value. Alternations compare exactly.
    pub fn canonical(&self, v: &str) -> Option<String> {
        self.canonical_folded(v, false)
    }

    /// As [`CType::canonical`]; with `fold`, a captured alternation compares
    /// with ASCII case folding and binds the template literal (SQ8).
    pub(crate) fn canonical_folded(&self, v: &str, fold: bool) -> Option<String> {
        match self {
            CType::Alternation(lits) => lits
                .iter()
                .find(|l| {
                    if fold {
                        l.eq_ignore_ascii_case(v)
                    } else {
                        *l == v
                    }
                })
                .cloned(),
            CType::Typed { tail: true, .. } => {
                let tail = v.rsplit_once('-').map_or(v, |(_, t)| t);
                self.canonical_whole(tail)
            }
            CType::Typed { .. } => self.canonical_whole(v),
        }
    }

    /// Canonicalizes a whole value, ignoring `@tail`. Title captures and scope
    /// values use this: they hold a value, not a segment.
    pub(crate) fn canonical_whole(&self, v: &str) -> Option<String> {
        match self {
            CType::Alternation(lits) => lits.iter().find(|l| *l == v).cloned(),
            CType::Typed {
                base, bounds, case, ..
            } => {
                let c = base.canonical(*bounds, v)?;
                Some(match case {
                    Some(CaseMod::Lower) => c.to_ascii_lowercase(),
                    Some(CaseMod::Upper) => c.to_ascii_uppercase(),
                    None => c,
                })
            }
        }
    }
}

/// `lit_char`: ASCII printable except `/ { } ( ) | ? # %` and space (§4.1).
pub(crate) fn is_lit_char(c: char) -> bool {
    c.is_ascii_graphic() && !matches!(c, '/' | '{' | '}' | '(' | ')' | '|' | '?' | '#' | '%')
}

/// ID-literal characters: `a-z`, `0-9`, `.`, `_`, `-` (§8.1).
pub(crate) fn is_id_literal_byte(b: u8) -> bool {
    b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
}

/// Parses `( literal { | literal } )` at the start of `text` and returns the
/// literals and the text after the closing parenthesis.
pub(crate) fn parse_alternation(text: &str) -> Result<(Vec<String>, &str), ErrorCode> {
    let inner_and_rest = text.strip_prefix('(').ok_or(ErrorCode::PathSyntax)?;
    let close = inner_and_rest.find(')').ok_or(ErrorCode::PathSyntax)?;
    let inner = &inner_and_rest[..close];
    let mut lits = Vec::new();
    for lit in inner.split('|') {
        check_literal(lit)?;
        lits.push(lit.to_owned());
    }
    if lits.len() > MAX_ALTERNATIVES {
        return Err(ErrorCode::TooManyAlternatives);
    }
    Ok((lits, &inner_and_rest[close + 1..]))
}

/// A path literal: one or more `lit_char`s, at most 64.
pub(crate) fn check_literal(lit: &str) -> Result<(), ErrorCode> {
    if lit.is_empty() || !lit.chars().all(is_lit_char) {
        return Err(ErrorCode::PathSyntax);
    }
    if lit.len() > MAX_LITERAL {
        return Err(ErrorCode::LiteralTooLong);
    }
    Ok(())
}

/// Parses `(n)` or `(n,m)` at the start of `text`. Numbers use the `int` form,
/// so a leading zero is a syntax error (SQ-V1). Huge numbers saturate, so
/// they fail as `bad_bounds` rather than overflow.
pub(crate) fn parse_bounds(text: &str) -> Option<((usize, usize), &str)> {
    let inner_and_rest = text.strip_prefix('(')?;
    let close = inner_and_rest.find(')')?;
    let inner = &inner_and_rest[..close];
    let number = |s: &str| -> Option<usize> {
        if s.is_empty()
            || !s.bytes().all(|b| b.is_ascii_digit())
            || (s.len() > 1 && s.starts_with('0'))
        {
            return None;
        }
        Some(s.bytes().fold(0usize, |n, b| {
            n.saturating_mul(10).saturating_add(usize::from(b - b'0'))
        }))
    };
    let bounds = match inner.split_once(',') {
        Some((a, b)) => (number(a)?, number(b)?),
        None => {
            let n = number(inner)?;
            (n, n)
        }
    };
    Some((bounds, &inner_and_rest[close + 1..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_need_an_opening_parenthesis() {
        // A `)` before any `(` once sliced text[1..0] and panicked.
        assert_eq!(parse_bounds(")#("), None);
        assert_eq!(parse_bounds("é)"), None);
        assert_eq!(parse_bounds("(2,4)x"), Some(((2, 4), "x")));
    }

    fn canon(ctype: &str, v: &str) -> Option<String> {
        CType::parse(ctype).unwrap().canonical(v)
    }

    #[test]
    fn types_accept_and_canonicalize() {
        assert_eq!(canon("int", "0").as_deref(), Some("0"));
        assert_eq!(canon("int", "007"), None);
        assert_eq!(canon("hex(4)", "BeEf").as_deref(), Some("beef"));
        assert_eq!(canon("token|lower", "A-b_C").as_deref(), Some("a-b_c"));
        assert_eq!(canon("name", ".."), None);
        assert_eq!(canon("jira_key", "proj_2-10").as_deref(), Some("PROJ_2-10"));
        assert_eq!(canon("jira_key", "PROJ-01"), None);
        assert_eq!(
            canon("uuid", "{0A1B2C3D-4E5F-6A7B-8C9D-0E1F2A3B4C5D}").as_deref(),
            Some("0a1b2c3d-4e5f-6a7b-8c9d-0e1f2a3b4c5d")
        );
        assert_eq!(canon("int@tail", "Plan-42").as_deref(), Some("42"));
        assert_eq!(canon("int@tail", "Plan-"), None);
        assert_eq!(canon("(edit|view)", "Edit"), None);
    }

    #[test]
    fn ctype_parts_are_checked_left_to_right() {
        assert_eq!(
            CType::parse("token@tail|lower"),
            Err(ErrorCode::TailNotAllowed)
        );
        assert_eq!(CType::parse("int@tail|lower"), Err(ErrorCode::PathSyntax));
        assert_eq!(CType::parse("int(0)@head"), Err(ErrorCode::BadBounds));
        assert_eq!(CType::parse("int(03)"), Err(ErrorCode::PathSyntax));
        assert_eq!(CType::parse("uuid(36)"), Err(ErrorCode::BadBounds));
        assert_eq!(CType::parse("int(1,19)"), CType::parse("int"));
    }
}

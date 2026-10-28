//! Private URL shapes (DESIGN §5.1).
//!
//! The extension turns URLs into shapes that hold route words and
//! placeholder classes, never a title or an identifier value. The host does
//! not trust those strings, so [`UrlShape::parse`] is an exact twin of the
//! extension's `isUrlShape` (`extension/site-patterns.ts`): the same
//! 2,048-character, 32-segment and 8-pair limits and the same literal rule.
//! `protocol/capture-site/shapes.json` pins both sides to one fixture.

use std::fmt;

use crate::policy::DraftPolicy;

/// Longest shape string, in characters.
pub const MAX_SHAPE_CHARS: usize = 2_048;
/// Most path segments in a shape.
pub const MAX_SHAPE_SEGMENTS: usize = 32;
/// Most query pairs in a shape.
pub const MAX_SHAPE_PAIRS: usize = 8;
/// Longest route word or query name kept as a literal.
pub const MAX_LITERAL_CHARS: usize = 20;

/// A placeholder class: what kind of value stood in a segment or a query
/// value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ShapeClass {
    Num,
    Key,
    Uuid,
    Hex32,
    Hex40,
    Hex64,
    TailHex32,
    Id,
    Slug,
}

impl ShapeClass {
    /// The name inside the braces, such as `num` for `{num}`.
    pub fn as_str(self) -> &'static str {
        match self {
            ShapeClass::Num => "num",
            ShapeClass::Key => "key",
            ShapeClass::Uuid => "uuid",
            ShapeClass::Hex32 => "hex32",
            ShapeClass::Hex40 => "hex40",
            ShapeClass::Hex64 => "hex64",
            ShapeClass::TailHex32 => "tailhex32",
            ShapeClass::Id => "id",
            ShapeClass::Slug => "slug",
        }
    }

    /// Parses a braced placeholder such as `{num}`.
    fn from_placeholder(part: &str) -> Option<ShapeClass> {
        let name = part.strip_prefix('{')?.strip_suffix('}')?;
        Some(match name {
            "num" => ShapeClass::Num,
            "key" => ShapeClass::Key,
            "uuid" => ShapeClass::Uuid,
            "hex32" => ShapeClass::Hex32,
            "hex40" => ShapeClass::Hex40,
            "hex64" => ShapeClass::Hex64,
            "tailhex32" => ShapeClass::TailHex32,
            "id" => ShapeClass::Id,
            "slug" => ShapeClass::Slug,
            _ => return None,
        })
    }

    /// The catalog's fixed example value for this class.
    fn example(self, policy: &DraftPolicy) -> &str {
        let fill = &policy.fill;
        match self {
            ShapeClass::Num => &fill.num,
            ShapeClass::Key => &fill.key,
            ShapeClass::Uuid => &fill.uuid,
            ShapeClass::Hex32 => &fill.hex32,
            ShapeClass::Hex40 => &fill.hex40,
            ShapeClass::Hex64 => &fill.hex64,
            ShapeClass::TailHex32 => &fill.tailhex32,
            ShapeClass::Id => &fill.id,
            ShapeClass::Slug => &fill.slug,
        }
    }
}

impl fmt::Display for ShapeClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{{{}}}", self.as_str())
    }
}

/// One path segment of a shape.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ShapePart {
    /// An empty segment, as in `/` or `//a/`.
    Empty,
    /// A route word that passed the literal rule.
    Literal(String),
    /// A placeholder class.
    Placeholder(ShapeClass),
}

/// A query parameter name.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum QueryName {
    /// A name that passed the literal rule.
    Literal(String),
    /// `{param}`: a name that failed the literal rule.
    Param,
}

/// A parsed URL shape. `path` always has at least one part; the root shape
/// `/` is one [`ShapePart::Empty`]. `query` keeps the order of the string.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UrlShape {
    pub path: Vec<ShapePart>,
    pub query: Vec<(QueryName, ShapeClass)>,
}

/// Why a string is not a URL shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapeError {
    /// Longer than [`MAX_SHAPE_CHARS`].
    TooLong,
    /// Does not start with `/`.
    NotAPath,
    /// More than one `?`.
    ExtraQuery,
    /// More than [`MAX_SHAPE_SEGMENTS`] path segments.
    TooManySegments,
    /// A path segment is neither empty, a literal nor a placeholder.
    BadSegment,
    /// More than [`MAX_SHAPE_PAIRS`] query pairs.
    TooManyPairs,
    /// A query pair is not `literal-or-{param}=placeholder`.
    BadPair,
}

impl fmt::Display for ShapeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ShapeError::TooLong => "the shape is longer than 2,048 characters",
            ShapeError::NotAPath => "the shape does not start with /",
            ShapeError::ExtraQuery => "the shape has more than one ?",
            ShapeError::TooManySegments => "the shape has more than 32 path segments",
            ShapeError::BadSegment => "a path segment is not a route word or a placeholder",
            ShapeError::TooManyPairs => "the shape has more than 8 query pairs",
            ShapeError::BadPair => "a query pair is not a name and a placeholder",
        })
    }
}

impl std::error::Error for ShapeError {}

/// The literal rule: 1 to 20 of `a-z`, `_` and `-`, at least one letter and
/// at most one separator.
fn is_literal(part: &str) -> bool {
    !part.is_empty()
        && part.len() <= MAX_LITERAL_CHARS
        && part
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b == b'_' || b == b'-')
        && part.bytes().any(|b| b.is_ascii_lowercase())
        && part.bytes().filter(|&b| b == b'_' || b == b'-').count() <= 1
}

impl UrlShape {
    /// Parses a shape string, accepting exactly what `isUrlShape` accepts.
    pub fn parse(value: &str) -> Result<UrlShape, ShapeError> {
        // `isUrlShape` counts UTF-16 code units. Any accepted shape is
        // ASCII, so a byte count refuses no shape that it accepts.
        if value.len() > MAX_SHAPE_CHARS && value.encode_utf16().count() > MAX_SHAPE_CHARS {
            return Err(ShapeError::TooLong);
        }
        if !value.starts_with('/') {
            return Err(ShapeError::NotAPath);
        }
        let mut halves = value.split('?');
        let path = halves.next().unwrap_or_default();
        let query = halves.next();
        if halves.next().is_some() {
            return Err(ShapeError::ExtraQuery);
        }
        let segments: Vec<&str> = path.split('/').skip(1).collect();
        if segments.len() > MAX_SHAPE_SEGMENTS {
            return Err(ShapeError::TooManySegments);
        }
        let path = segments
            .into_iter()
            .map(|part| {
                if part.is_empty() {
                    Ok(ShapePart::Empty)
                } else if is_literal(part) {
                    Ok(ShapePart::Literal(part.to_owned()))
                } else {
                    ShapeClass::from_placeholder(part)
                        .map(ShapePart::Placeholder)
                        .ok_or(ShapeError::BadSegment)
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        let Some(query) = query else {
            return Ok(UrlShape {
                path,
                query: Vec::new(),
            });
        };
        let pairs: Vec<&str> = query.split('&').collect();
        if pairs.len() > MAX_SHAPE_PAIRS {
            return Err(ShapeError::TooManyPairs);
        }
        let query = pairs
            .into_iter()
            .map(|pair| {
                let mut sides = pair.split('=');
                let name = sides.next().unwrap_or_default();
                let class = sides.next().and_then(ShapeClass::from_placeholder);
                let (Some(class), None) = (class, sides.next()) else {
                    return Err(ShapeError::BadPair);
                };
                let name = if is_literal(name) {
                    QueryName::Literal(name.to_owned())
                } else if name == "{param}" {
                    QueryName::Param
                } else {
                    return Err(ShapeError::BadPair);
                };
                Ok((name, class))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(UrlShape { path, query })
    }

    /// An illustrative URL on `origin`: every placeholder becomes the
    /// catalog's fixed example of its class and every `{param}` name becomes
    /// `param`. It is never a URL the user visited.
    pub fn fill(&self, origin: &str, fill: &DraftPolicy) -> String {
        let mut out = String::from(origin);
        for part in &self.path {
            out.push('/');
            match part {
                ShapePart::Empty => {}
                ShapePart::Literal(word) => out.push_str(word),
                ShapePart::Placeholder(class) => out.push_str(class.example(fill)),
            }
        }
        for (index, (name, class)) in self.query.iter().enumerate() {
            out.push(if index == 0 { '?' } else { '&' });
            out.push_str(match name {
                QueryName::Literal(word) => word,
                QueryName::Param => "param",
            });
            out.push('=');
            out.push_str(class.example(fill));
        }
        out
    }
}

impl fmt::Display for UrlShape {
    /// The canonical string, byte-identical to the one that was parsed.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for part in &self.path {
            f.write_str("/")?;
            match part {
                ShapePart::Empty => {}
                ShapePart::Literal(word) => f.write_str(word)?,
                ShapePart::Placeholder(class) => write!(f, "{class}")?,
            }
        }
        for (index, (name, class)) in self.query.iter().enumerate() {
            f.write_str(if index == 0 { "?" } else { "&" })?;
            match name {
                QueryName::Literal(word) => f.write_str(word)?,
                QueryName::Param => f.write_str("{param}")?,
            }
            write!(f, "={class}")?;
        }
        Ok(())
    }
}

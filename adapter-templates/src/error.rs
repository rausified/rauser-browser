//! Validation error codes and locations.
//!
//! The codes are the complete format-1 enum from
//! `protocol/adapter-templates/README.md`; the shared vectors pin each one.

use std::fmt;

use serde::Serialize;

macro_rules! codes {
    ($(#[$meta:meta])* $name:ident { $($variant:ident => $text:literal,)* }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum $name {
            $($variant,)*
        }

        impl $name {
            pub fn as_str(self) -> &'static str {
                match self {
                    $($name::$variant => $text,)*
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.as_str())
            }
        }
    };
}

codes! {
    /// A config validation failure (§10.3).
    ErrorCode {
        MissingFormat => "missing_format",
        InvalidFormat => "invalid_format",
        UnsupportedFormat => "unsupported_format",
        TooManyAdapters => "too_many_adapters",
        UnknownKey => "unknown_key",
        MissingKey => "missing_key",
        WrongType => "wrong_type",
        BadAdapterType => "bad_adapter_type",
        ReservedAdapterType => "reserved_adapter_type",
        BadOrigin => "bad_origin",
        BadPathCase => "bad_path_case",
        NoRules => "no_rules",
        TooManyRules => "too_many_rules",
        PathSyntax => "path_syntax",
        BadCaptureName => "bad_capture_name",
        ReservedCaptureName => "reserved_capture_name",
        UnknownCaptureType => "unknown_capture_type",
        BadBounds => "bad_bounds",
        CaseModNotAllowed => "case_mod_not_allowed",
        TailNotAllowed => "tail_not_allowed",
        BadCaptureLiteral => "bad_capture_literal",
        RestNotLast => "rest_not_last",
        OptionalMisplaced => "optional_misplaced",
        NoLiteralSegment => "no_literal_segment",
        DuplicateCapture => "duplicate_capture",
        TooManySegments => "too_many_segments",
        TooManyAlternatives => "too_many_alternatives",
        LiteralTooLong => "literal_too_long",
        TemplateTooLong => "template_too_long",
        TooManyQuery => "too_many_query",
        TooManyAbsent => "too_many_absent",
        BadQueryKey => "bad_query_key",
        BadQueryValue => "bad_query_value",
        OptionalInQuery => "optional_in_query",
        QueryKeyConflict => "query_key_conflict",
        BadMode => "bad_mode",
        CaptureTypeConflict => "capture_type_conflict",
        ExtraCapture => "extra_capture",
        BadModesValue => "bad_modes_value",
        ModeMissingMapping => "mode_missing_mapping",
        ModeNotAlternation => "mode_not_alternation",
        ModeInId => "mode_in_id",
        IdSyntax => "id_syntax",
        IdReservedScheme => "id_reserved_scheme",
        IdUnknownCapture => "id_unknown_capture",
        IdOptionalCapture => "id_optional_capture",
        SharedOriginUnscoped => "shared_origin_unscoped",
        ScopeAndUnscoped => "scope_and_unscoped",
        UnscopedNotShared => "unscoped_not_shared",
        BadUnscoped => "bad_unscoped",
        ScopeEmptyList => "scope_empty_list",
        TooManyScopeValues => "too_many_scope_values",
        ScopeCaptureNotBound => "scope_capture_not_bound",
        ScopeBadValue => "scope_bad_value",
        TitleTooLong => "title_too_long",
        TitleSyntax => "title_syntax",
        TitleUnboundedCapture => "title_unbounded_capture",
        TitleCaptureSwallows => "title_capture_swallows",
        TitleUnknownCapture => "title_unknown_capture",
        TitleTypeMismatch => "title_type_mismatch",
        TooManyTitles => "too_many_titles",
        EmptyTitleList => "empty_title_list",
        BadAlias => "bad_alias",
        AliasCaptureNotId => "alias_capture_not_id",
        AliasCaptureNotInTitle => "alias_capture_not_in_title",
        AliasTypeMismatch => "alias_type_mismatch",
        TooManyRefs => "too_many_refs",
        BadRef => "bad_ref",
        BadRefFind => "bad_ref_find",
    }
}

codes! {
    /// A Tier 2 draft rejection (§10.4).
    DraftErrorCode {
        ForbiddenKey => "draft_forbidden_key",
        NoScopeSource => "draft_no_scope_source",
        ScopeConflict => "draft_scope_conflict",
        NoLeadingLiteral => "draft_no_leading_literal",
        RestSecond => "draft_rest_second",
        IdNeedsTypedCapture => "draft_id_needs_typed_capture",
        SearchCapture => "draft_search_capture",
        MatchesProbe => "draft_matches_probe",
        Overlap => "draft_overlap",
        BlockedRoute => "draft_blocked_route",
        PagingCapture => "draft_paging_capture",
        InvalidRule => "draft_invalid_rule",
        ScopePending => "draft_scope_pending",
        ScopeChoice => "draft_scope_choice",
    }
}

/// The template string a [`ValidationError::position`] counts characters in.
/// The adapter, rule, and title entry come from the error's other fields.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    /// The rule's `path`.
    Path,
    /// The rule's `query` value under this key.
    Query(String),
    /// The rule's `absent` entry at this index.
    Absent(usize),
    /// The adapter's `id`.
    Id,
    /// The title entry named by `entry`.
    Title,
    /// The alias's `capture` (`tab_title`) or `value` (`meta`).
    Alias,
}

impl fmt::Display for Field {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Field::Path => f.write_str("the path"),
            Field::Query(key) => write!(f, "the query value for {key:?}"),
            Field::Absent(i) => write!(f, "absent entry {i}"),
            Field::Id => f.write_str("the id"),
            Field::Title => f.write_str("the title"),
            Field::Alias => f.write_str("the alias"),
        }
    }
}

/// The first validation error in check order, with its location (§10.3).
/// `position` is a character offset into the template string named by
/// `field`: the start of the offending segment, part, or capture, or of the
/// first bad character in a literal. Errors about a whole template report 0,
/// and errors about a table or list rather than a template string carry no
/// position. The shared vectors do not pin positions (SQ-V9).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ValidationError {
    pub code: ErrorCode,
    pub adapter: Option<usize>,
    pub rule: Option<usize>,
    pub entry: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<Field>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<usize>,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code.as_str())?;
        if let Some(a) = self.adapter {
            write!(f, " in adapter {a}")?;
        }
        if let Some(r) = self.rule {
            write!(f, ", rule {r}")?;
        }
        if let Some(e) = self.entry {
            write!(f, ", entry {e}")?;
        }
        match (&self.field, self.position) {
            (Some(field), Some(p)) => write!(f, " at character {p} of {field}")?,
            (None, Some(p)) => write!(f, " at character {p}")?,
            (Some(field), None) => write!(f, " in {field}")?,
            (None, None) => {}
        }
        Ok(())
    }
}

impl std::error::Error for ValidationError {}

//! The adapter-template engine (`adapter-templates.md`, DESIGN §12.2 step 6.2).
//!
//! Adapters recognize artifacts by URL shape with a small template language
//! instead of regular expressions. This crate is the host's authoritative
//! implementation: it validates a config into a [`Document`] in the README
//! check order, evaluates URLs and tab titles against it, scans and resolves
//! refs, reports conservative rule overlaps, and judges Tier 2 drafts.
//!
//! Every step is linear in its input and none backtracks; nothing here
//! compiles a template to a regular expression (§10.2).

#![forbid(unsafe_code)]

mod ctype;
mod document;
mod draft;
mod error;
mod eval;
mod overlap;
mod path;
mod policy;
mod title;
mod value;

pub use ctype::{BaseType, CType, CaseMod};
pub use document::{Adapter, Document, MAX_ADAPTERS, MAX_RULES, Mode, SHARED_ORIGINS};
pub use draft::{DraftOutcome, Scope, check_draft};
pub use error::{DraftErrorCode, ErrorCode, Field, ValidationError};
pub use eval::{
    MAX_ID_BYTES, MAX_REFS, MAX_URL_CHARS, Match, NoMatch, Ref, ResolvedRef, TitleMatch, scan_refs,
};
pub use overlap::OverlapPair;
pub use path::prepare_segments;
pub use policy::{DraftPolicy, Fill, SharedOrigin};
pub use value::Value;

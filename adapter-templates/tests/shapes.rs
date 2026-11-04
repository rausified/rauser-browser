//! Pins the host's URL-shape parser to the shared shape fixture
//! (`protocol/capture-site/shapes.json`), which the extension's in-page
//! shaper (`inspectSitePatterns`) and `isUrlShape` also run against.

use adapter_templates::shape::{QueryName, ShapeClass, ShapePart, UrlShape};
use adapter_templates::{DraftPolicy, Fill};
use serde_json::Value as Json;

const SHAPES: &str = include_str!("../../protocol/capture-site/shapes.json");
const STARTER_SHAPES: &str = include_str!("../../protocol/capture-site/starter-shapes.json");
const CATALOG: &str = include_str!("../../protocol/capture-site/catalog.json");

const ALL_CLASSES: [ShapeClass; 9] = [
    ShapeClass::Num,
    ShapeClass::Key,
    ShapeClass::Uuid,
    ShapeClass::Hex32,
    ShapeClass::Hex40,
    ShapeClass::Hex64,
    ShapeClass::TailHex32,
    ShapeClass::Id,
    ShapeClass::Slug,
];

fn fixture() -> Json {
    serde_json::from_str(SHAPES).expect("shapes.json parses")
}

fn starter_fixture() -> Json {
    serde_json::from_str(STARTER_SHAPES).expect("starter-shapes.json parses")
}

/// Expands a string or a `{"$build": [...]}` of strings and
/// `{"repeat", "times"}` parts.
fn built(value: &Json) -> String {
    if let Some(s) = value.as_str() {
        return s.to_owned();
    }
    let parts = value["$build"].as_array().expect("$build array");
    parts
        .iter()
        .map(|part| match part {
            Json::String(s) => s.clone(),
            Json::Object(o) => {
                let unit = o["repeat"].as_str().expect("repeat string");
                let times = o["times"].as_u64().expect("times") as usize;
                unit.repeat(times)
            }
            other => panic!("bad builder part {other}"),
        })
        .collect()
}

fn section_rows(source: &Json, section: &str) -> Vec<(String, String)> {
    source[section]
        .as_array()
        .unwrap_or_else(|| panic!("{section} array"))
        .iter()
        .map(|row| {
            let note = row["note"].as_str().expect("note").to_owned();
            (note, built(&row["shape"]))
        })
        .collect()
}

/// `section`'s rows of `shapes.json` followed by those of
/// `starter-shapes.json` (the 15 starter rows this moved out of
/// `shapes.json`, `protocol/starters/README.md` "Recorded samples").
fn rows(section: &str) -> Vec<(String, String)> {
    let mut all = section_rows(&fixture(), section);
    all.extend(section_rows(&starter_fixture(), section));
    all
}

/// A policy whose fill values come from the host catalog; the drafting
/// lists are irrelevant to filling.
fn policy() -> DraftPolicy {
    let catalog: Json = serde_json::from_str(CATALOG).expect("catalog.json parses");
    let fill = &catalog["fill"];
    let value = |class: &str| fill[class].as_str().expect("fill value").to_owned();
    DraftPolicy {
        probe_paths: Vec::new(),
        search_keys: Vec::new(),
        paging_keys: Vec::new(),
        blocked_route_words: Vec::new(),
        secret_query_keys: Vec::new(),
        secret_query_key_parts: Vec::new(),
        secret_query_key_suffixes: Vec::new(),
        mode_words: Vec::new(),
        fill: Fill {
            num: value("num"),
            key: value("key"),
            slug: value("slug"),
            uuid: value("uuid"),
            hex32: value("hex32"),
            hex40: value("hex40"),
            hex64: value("hex64"),
            tailhex32: value("tailhex32"),
            id: value("id"),
        },
        starter_types: Vec::new(),
        shared: Vec::new(),
    }
}

#[test]
fn shape_fixture_round_trips_through_rust_parser() {
    let accept = rows("accept");
    assert!(
        accept.len() >= 30,
        "expected the shared fixture, saw {}",
        accept.len()
    );
    let mut failures = Vec::new();
    for (note, shape) in &accept {
        match UrlShape::parse(shape) {
            Ok(parsed) if parsed.to_string() == *shape => {
                // Parsing the canonical string again is a fixed point.
                assert_eq!(UrlShape::parse(&parsed.to_string()).as_ref(), Ok(&parsed));
            }
            Ok(parsed) => failures.push(format!("{note}: {shape} displayed as {parsed}")),
            Err(error) => failures.push(format!("{note}: {shape} refused: {error}")),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn starter_shape_rows_all_name_a_doc() {
    // starter-shapes.json's rows are the ones this task moved out of
    // shapes.json (protocol/starters/README.md "Recorded samples"); each
    // one's note still names the doc page `renderStarterShapes` suffixed it
    // with, and shapes.json keeps none of them. The emptiness checks guard
    // against both loops passing vacuously if starter-shapes.json ever
    // shipped with no rows.
    let starter_rows = section_rows(&starter_fixture(), "accept");
    assert!(!starter_rows.is_empty(), "starter-shapes.json accept rows");
    for (note, _) in &starter_rows {
        assert!(note.contains("(docs/adapters/"), "{note}");
    }
    let shape_rows = section_rows(&fixture(), "accept");
    assert!(!shape_rows.is_empty(), "shapes.json accept rows");
    for (note, _) in &shape_rows {
        assert!(!note.contains("(docs/adapters/"), "{note}");
    }
}

#[test]
fn shape_parser_rejects_what_is_url_shape_rejects() {
    let reject = rows("reject");
    assert!(
        reject.len() >= 20,
        "expected the shared fixture, saw {}",
        reject.len()
    );
    let accepted: Vec<String> = reject
        .iter()
        .filter(|(_, shape)| UrlShape::parse(shape).is_ok())
        .map(|(note, shape)| format!("{note}: {}", &shape[..shape.len().min(80)]))
        .collect();
    assert!(accepted.is_empty(), "accepted:\n{}", accepted.join("\n"));
}

#[test]
fn shape_parser_parts_follow_the_canonical_string() {
    let parsed = UrlShape::parse("//jira/{key}/x_y?a={num}&{param}={tailhex32}").unwrap();
    assert_eq!(
        parsed.path,
        vec![
            ShapePart::Empty,
            ShapePart::Literal("jira".into()),
            ShapePart::Placeholder(ShapeClass::Key),
            ShapePart::Literal("x_y".into()),
        ]
    );
    assert_eq!(
        parsed.query,
        vec![
            (QueryName::Literal("a".into()), ShapeClass::Num),
            (QueryName::Param, ShapeClass::TailHex32),
        ]
    );
    let root = UrlShape::parse("/").unwrap();
    assert_eq!(root.path, vec![ShapePart::Empty]);
    assert!(root.query.is_empty());
    assert_eq!(root.to_string(), "/");
}

#[test]
fn filled_shapes_use_fixed_class_examples() {
    let policy = policy();
    let fill = |shape: &str| {
        UrlShape::parse(shape)
            .unwrap()
            .fill("https://x.test", &policy)
    };
    assert_eq!(fill("/a/{num}"), "https://x.test/a/123456");
    assert_eq!(fill("/browse/{key}"), "https://x.test/browse/ABC-123");
    assert_eq!(fill("/pages/{slug}"), "https://x.test/pages/example-page");
    assert_eq!(fill("/"), "https://x.test/");
    assert_eq!(fill("//a/"), "https://x.test//a/");
    assert_eq!(
        fill("/d/{id}/edit?view={num}&{param}={slug}"),
        "https://x.test/d/example123/edit?view=123456&param=example-page"
    );
    for class in ALL_CLASSES {
        let expected = match class {
            ShapeClass::Num => &policy.fill.num,
            ShapeClass::Key => &policy.fill.key,
            ShapeClass::Uuid => &policy.fill.uuid,
            ShapeClass::Hex32 => &policy.fill.hex32,
            ShapeClass::Hex40 => &policy.fill.hex40,
            ShapeClass::Hex64 => &policy.fill.hex64,
            ShapeClass::TailHex32 => &policy.fill.tailhex32,
            ShapeClass::Id => &policy.fill.id,
            ShapeClass::Slug => &policy.fill.slug,
        };
        let shape = format!("/a/{{{}}}?b={{{}}}", class.as_str(), class.as_str());
        assert_eq!(
            fill(&shape),
            format!("https://x.test/a/{expected}?b={expected}")
        );
    }
}

#[test]
fn no_parsed_shape_holds_a_digit_outside_a_placeholder_or_an_uppercase_letter() {
    let clean = |word: &str| {
        !word
            .chars()
            .any(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
    };
    let mut literals = 0;
    for (note, shape) in rows("accept") {
        let parsed = UrlShape::parse(&shape).unwrap_or_else(|e| panic!("{note}: {e}"));
        for part in &parsed.path {
            if let ShapePart::Literal(word) = part {
                literals += 1;
                assert!(clean(word), "{note}: path literal {word}");
            }
        }
        for (name, _) in &parsed.query {
            if let QueryName::Literal(word) = name {
                literals += 1;
                assert!(clean(word), "{note}: query name {word}");
            }
        }
    }
    assert!(literals > 0);
    // The parser refuses what a clean literal forbids rather than
    // smuggling it through as a literal.
    for shape in ["/A", "/a1", "/a?B={num}", "/a?b2={num}"] {
        assert!(UrlShape::parse(shape).is_err(), "{shape}");
    }
}

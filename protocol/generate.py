#!/usr/bin/env python3
"""Generate the Rust and TypeScript wire types from schema.json.

This intentionally supports only the small JSON Schema subset used by this protocol.
An unsupported schema feature fails generation instead of silently producing an
incomplete type. No package installation is needed to regenerate the files.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path


HERE = Path(__file__).resolve().parent
SCHEMA_PATH = HERE / "schema.json"
RUST_PATH = HERE / "src" / "generated.rs"
TYPESCRIPT_PATH = HERE / "ts" / "generated.ts"


def fail(message: str) -> None:
    raise ValueError(message)


def ref_name(value: dict) -> str:
    reference = value.get("$ref")
    if not isinstance(reference, str) or not reference.startswith("#/$defs/"):
        fail(f"unsupported reference: {value!r}")
    return reference.removeprefix("#/$defs/")


def words_to_pascal(value: str) -> str:
    if not re.fullmatch(r"[a-z][a-z0-9]*(?:_[a-z0-9]+)*", value):
        fail(f"unsupported enum value: {value!r}")
    return "".join(word.capitalize() for word in value.split("_"))


def shape_type(shape: dict, target: str) -> str:
    if "$ref" in shape:
        return ref_name(shape)
    if shape.get("type") == "array":
        item = shape.get("items")
        if not isinstance(item, dict):
            fail(f"unsupported array item: {shape!r}")
        inner = shape_type(item, target)
        if target == "rust":
            return f"Vec<{inner}>"
        return f"({inner})[]" if " | " in inner else f"{inner}[]"
    if "oneOf" in shape:
        alternatives = shape["oneOf"]
        if len(alternatives) == 2 and alternatives[1] == {"type": "null"}:
            inner = shape_type(alternatives[0], target)
            return f"Option<{inner}>" if target == "rust" else f"{inner} | null"
        fail(f"unsupported union: {shape!r}")
    if "const" in shape:
        value = shape["const"]
        if isinstance(value, int) and not isinstance(value, bool):
            return "u32" if target == "rust" else str(value)
        if isinstance(value, str):
            if target == "typescript":
                return json.dumps(value)
            fail("string constants are only supported on TypeScript tags")
    if shape.get("type") == "object" and set(shape) == {"type"}:
        return "serde_json::Value" if target == "rust" else "Record<string, unknown>"
    types = {
        "string": ("String", "string"),
        "boolean": ("bool", "boolean"),
        "integer": ("u32", "number"),
    }
    kind = shape.get("type")
    if kind not in types:
        fail(f"unsupported property shape: {shape!r}")
    return types[kind][0 if target == "rust" else 1]


def optional_inner(shape: dict) -> dict:
    """The value shape of an optional property: a nullable one's non-null
    alternative, since a missing key and null both read as no value."""
    alternatives = shape.get("oneOf")
    if isinstance(alternatives, list) and len(alternatives) == 2 and alternatives[1] == {"type": "null"}:
        return alternatives[0]
    return shape


def is_optional(definition: dict, field: str) -> bool:
    return field not in definition.get("required", [])


def variants(definitions: dict, name: str) -> list[str]:
    return [ref_name(item) for item in definitions[name]["oneOf"]]


def is_union(definition: dict) -> bool:
    """A named union of object or enum definitions, told apart by shape."""
    return "oneOf" in definition and set(definition) <= {"oneOf", "description"}


def union_members(definitions: dict, name: str) -> list[str]:
    members = variants(definitions, name)
    if len(members) < 2:
        fail(f"{name}: a union needs at least two members")
    for member in members:
        target = definitions.get(member)
        if target is None or is_union(target) or target.get("type") not in ("object", "string"):
            fail(f"{name}: union member {member} must be an object or enum definition")
    return members


def variant_name(struct_name: str) -> str:
    for suffix in ("Request", "Response"):
        if struct_name.endswith(suffix):
            return struct_name[: -len(suffix)]
    return struct_name


def variant_tag(struct_name: str) -> str:
    name = variant_name(struct_name)
    return re.sub(r"(?<!^)(?=[A-Z])", "_", name).lower()


def verify_schema(definitions: dict) -> int:
    message_names = variants(definitions, "Request") + variants(definitions, "Response")
    versions = set()
    tags = set()
    for name, definition in definitions.items():
        if definition.get("type") != "object":
            continue
        properties = definition.get("properties", {})
        if definition.get("additionalProperties") is not False:
            fail(f"{name}: object must reject unknown properties")
        required = definition.get("required", [])
        if not set(required) <= set(properties) or len(set(required)) != len(required):
            fail(f"{name}: required names an unknown or repeated property")
        for field in set(properties) - set(required):
            # An optional property is an additive field an older peer may
            # omit; message headers never are.
            if field in ("type", "protocol_version", "request_id"):
                fail(f"{name}: {field} must be required")
        if name in message_names:
            if properties.get("protocol_version", {}).get("type") != "integer":
                fail(f"{name}: protocol_version must be an integer")
            versions.add(properties["protocol_version"].get("const"))
            tag = properties.get("type", {}).get("const")
            if not isinstance(tag, str) or tag in tags:
                fail(f"{name}: duplicate or missing message type")
            if tag != variant_tag(name):
                fail(f"{name}: tag {tag!r} differs from generated Rust tag")
            tags.add(tag)
            if "request_id" not in properties:
                fail(f"{name}: missing request_id")
        elif "type" in properties:
            fail(f"{name}: only messages may have a type tag")
    if len(versions) != 1 or not isinstance(next(iter(versions)), int):
        fail("messages must share one integer protocol_version")
    return next(iter(versions))


def render_rust(definitions: dict, version: int) -> str:
    output = [
        "// Generated by protocol/generate.py from protocol/schema.json. Do not edit.",
        "use serde::{Deserialize, Serialize};",
        "",
        f"pub const PROTOCOL_VERSION: u32 = {version};",
        "",
        "// A nullable JSON Schema property is still required on the wire.",
        "// Serde otherwise treats a missing Option field as None.",
        "fn deserialize_required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>",
        "where",
        "    D: serde::Deserializer<'de>,",
        "    T: Deserialize<'de>,",
        "{",
        "    Option::<T>::deserialize(deserializer)",
        "}",
        "",
    ]
    for name, definition in definitions.items():
        if name in ("Request", "Response"):
            continue
        if definition.get("type") == "string" and "enum" in definition:
            output.extend(
                [
                    "#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]",
                    '#[serde(rename_all = "snake_case")]',
                    f"pub enum {name} {{",
                    *[f"    {words_to_pascal(value)}," for value in definition["enum"]],
                    "}",
                    "",
                ]
            )
            continue
        if is_union(definition):
            # Members reject unknown fields and values, so at most one of
            # them reads any given JSON value.
            output.extend(
                [
                    "#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]",
                    "#[serde(untagged)]",
                    f"pub enum {name} {{",
                    *[f"    {member}({member})," for member in union_members(definitions, name)],
                    "}",
                    "",
                ]
            )
            continue
        if definition.get("type") != "object":
            fail(f"unsupported definition: {name}")
        output.extend(
            [
                "#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]",
                "#[serde(deny_unknown_fields)]",
                f"pub struct {name} {{",
            ]
        )
        for field, shape in definition["properties"].items():
            if field == "type":
                continue
            if is_optional(definition, field):
                # Additive within a version: missing (or null) reads as None,
                # and None is not written, so an older peer never sees it.
                inner = shape_type(optional_inner(shape), "rust")
                output.append('    #[serde(default, skip_serializing_if = "Option::is_none")]')
                output.append(f"    pub {field}: Option<{inner}>,")
                continue
            if "oneOf" in shape:
                output.append('    #[serde(deserialize_with = "deserialize_required_nullable")]')
            output.append(f"    pub {field}: {shape_type(shape, 'rust')},")
        output.extend(["}", ""])
    for name in ("Request", "Response"):
        items = variants(definitions, name)
        output.extend(
            [
                "#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]",
                '#[serde(tag = "type", rename_all = "snake_case")]',
                # One message is decoded, handled, and dropped at a time, so
                # boxing the config-carrying variants would buy nothing.
                "#[allow(clippy::large_enum_variant)]",
                f"pub enum {name} {{",
            ]
        )
        output.extend(f"    {variant_name(item)}({item})," for item in items)
        output.extend(["}", "", f"impl {name} {{"])
        for field, return_type, expression in (
            ("protocol_version", "u32", "value.protocol_version"),
            ("request_id", "&str", "&value.request_id"),
        ):
            output.extend(
                [
                    f"    pub fn {field}(&self) -> {return_type} {{",
                    "        match self {",
                    *[
                        f"            Self::{variant_name(item)}(value) => {expression},"
                        for item in items
                    ],
                    "        }",
                    "    }",
                    "",
                ]
            )
        output.pop()  # rustfmt does not leave a blank line before the impl brace
        output.extend(["}", ""])
    return "\n".join(output)


def render_typescript(definitions: dict, version: int) -> str:
    output = [
        "// Generated by protocol/generate.py from protocol/schema.json. Do not edit.",
        f"export const PROTOCOL_VERSION = {version} as const;",
        "",
    ]
    for name, definition in definitions.items():
        if name in ("Request", "Response"):
            continue
        if definition.get("type") == "string" and "enum" in definition:
            values = " | ".join(json.dumps(value) for value in definition["enum"])
            output.extend([f"export type {name} = {values};", ""])
            continue
        if is_union(definition):
            members = " | ".join(union_members(definitions, name))
            output.extend([f"export type {name} = {members};", ""])
            continue
        if definition.get("type") != "object":
            fail(f"unsupported definition: {name}")
        output.append(f"export interface {name} {{")
        for field, shape in definition["properties"].items():
            marker = "?" if is_optional(definition, field) else ""
            output.append(f"  {field}{marker}: {shape_type(shape, 'typescript')};")
        output.extend(["}", ""])
    for name in ("Request", "Response"):
        members = " | ".join(variants(definitions, name))
        output.extend([f"export type {name} = {members};", ""])
    return "\n".join(output)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="fail if generated files differ")
    args = parser.parse_args()
    schema = json.loads(SCHEMA_PATH.read_text(encoding="utf-8"))
    definitions = schema["$defs"]
    version = verify_schema(definitions)
    outputs = {
        RUST_PATH: render_rust(definitions, version),
        TYPESCRIPT_PATH: render_typescript(definitions, version),
    }
    stale = []
    for path, content in outputs.items():
        if args.check:
            if not path.exists() or path.read_text(encoding="utf-8") != content:
                stale.append(path)
        else:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content, encoding="utf-8")
    if stale:
        for path in stale:
            print(f"out of date: {path.relative_to(HERE.parent)}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (KeyError, ValueError) as error:
        print(f"invalid protocol schema: {error}", file=sys.stderr)
        sys.exit(1)

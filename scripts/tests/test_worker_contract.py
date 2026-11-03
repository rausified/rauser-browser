"""worker-contract.json is valid against worker-contract.schema.json and agrees with the tools.

The shared worker driver reads the contract (AGENTS.md "Worker contract"), so
a typo there misleads every worker. CI's Python has no jsonschema package, so
this validates with the small subset of JSON Schema the schema uses, and
checks that subset rejects what it should.
"""

from __future__ import annotations

import contextlib
import io
import json
import pathlib
import sys
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

import check_current_state  # noqa: E402
import work_queue  # noqa: E402

SUPPORTED = {"$schema", "$id", "title", "description", "type", "additionalProperties", "required",
             "properties", "const", "enum", "items", "minItems", "uniqueItems", "minLength"}
TYPES = {"object": dict, "array": list, "string": str}


def errors(value: object, schema: dict, where: str = "$") -> list[str]:
    unknown = set(schema) - SUPPORTED
    if unknown:
        raise AssertionError(f"schema at {where} uses unsupported keywords {sorted(unknown)}")
    found: list[str] = []
    kind = schema.get("type")
    if kind and not isinstance(value, TYPES[kind]):
        return [f"{where}: expected {kind}"]
    if "const" in schema and (value != schema["const"] or type(value) is not type(schema["const"])):
        found.append(f"{where}: must be {schema['const']!r}")
    if "enum" in schema and value not in schema["enum"]:
        found.append(f"{where}: must be one of {schema['enum']}")
    if isinstance(value, str) and len(value) < schema.get("minLength", 0):
        found.append(f"{where}: too short")
    if isinstance(value, list):
        if len(value) < schema.get("minItems", 0):
            found.append(f"{where}: too few items")
        if schema.get("uniqueItems") and len({json.dumps(item) for item in value}) != len(value):
            found.append(f"{where}: items repeat")
        for index, item in enumerate(value):
            found += errors(item, schema.get("items", {}), f"{where}[{index}]")
    if isinstance(value, dict):
        for key in schema.get("required", []):
            if key not in value:
                found.append(f"{where}: missing {key}")
        properties = schema.get("properties", {})
        for key, item in value.items():
            if key in properties:
                found += errors(item, properties[key], f"{where}.{key}")
            elif schema.get("additionalProperties") is False:
                found.append(f"{where}: unexpected {key}")
    return found


class WorkerContractTests(unittest.TestCase):
    def setUp(self) -> None:
        self.contract = json.loads((ROOT / "worker-contract.json").read_text(encoding="utf-8"))
        self.schema = json.loads((ROOT / "worker-contract.schema.json").read_text(encoding="utf-8"))

    def test_the_contract_matches_its_schema(self) -> None:
        self.assertEqual(errors(self.contract, self.schema), [])

    def test_the_validator_rejects_broken_contracts(self) -> None:
        for change, message in (
            (lambda c: c.update(version=2), "must be 1"),
            (lambda c: c.update(version=True), "must be 1"),
            (lambda c: c.pop("queue"), "missing queue"),
            (lambda c: c["merge"].update(method="api"), "one of"),
            (lambda c: c["queue"].update(transport="git-file"), "one of"),
            (lambda c: c.update(never_commit=[]), "too few"),
            (lambda c: c.update(extra=1), "unexpected extra"),
            (lambda c: c["queue"].update(sync=""), "too short"),
        ):
            contract = json.loads(json.dumps(self.contract))
            change(contract)
            with self.subTest(message=message):
                self.assertTrue(any(message in e for e in errors(contract, self.schema)), errors(contract, self.schema))

    def test_never_commit_is_what_ci_enforces(self) -> None:
        self.assertEqual(tuple(self.contract["never_commit"]), check_current_state.QUEUE_FILES)

    def test_the_queue_commands_name_real_subcommands(self) -> None:
        for key in ("sync", "export", "backlog"):
            command = self.contract["queue"][key].split()
            self.assertEqual(command[0], "scripts/dev/queue")
            self.assertTrue((ROOT / command[0]).is_file())
            with self.assertRaises(SystemExit) as raised, contextlib.redirect_stdout(io.StringIO()):
                work_queue.main([command[1], "--help"])
            self.assertEqual(raised.exception.code, 0)
        self.assertEqual(self.contract["queue"]["transport"], "bd-dolt")

    def test_the_sync_exit_codes_are_the_tools(self) -> None:
        codes = {int(code): meaning for code, meaning in self.contract["queue"]["sync_exit_codes"].items()}
        self.assertEqual(set(codes), {0, 1, 2, 3, 4})
        self.assertEqual(set(codes) - {0, 1}, set(work_queue.SYNC_EXITS) - {1})
        self.assertEqual((codes[2], codes[3], codes[4]), ("conflict", "retry", "stuck"))
        self.assertIn("bd bootstrap", self.contract["queue"]["bootstrap"])


if __name__ == "__main__":
    unittest.main()

"""Converts research/blind/figma/adapters.toml to the JSON document shape the engines take."""
import json, sys, tomllib
with open(sys.argv[1], "rb") as f:
    doc = tomllib.load(f)
out = {"format": doc["adapters"]["format"], "artifacts": doc["artifacts"]}
json.dump(out, sys.stdout, ensure_ascii=False, indent=1)

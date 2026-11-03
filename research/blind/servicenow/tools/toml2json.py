"""Throwaway: convert the blind adapters.toml to the engines' JSON document
shape ({"format": 1, "artifacts": [...]}). Usage: python3 -I toml2json.py IN OUT"""
import json, sys, tomllib
with open(sys.argv[1], "rb") as f:
    doc = tomllib.load(f)
out = {"format": doc["adapters"]["format"], "artifacts": doc["artifacts"]}
with open(sys.argv[2], "w") as f:
    json.dump(out, f, indent=1)

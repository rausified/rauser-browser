#!/usr/bin/env python3
"""Throwaway: runs adapters.toml + vectors.json through an engine harness.
Usage: validate.py <engine-command...>   (reads JSON Lines on stdin, as
scripts/adapter-templates-eval.mjs and `brauser adapters eval` do)."""
import json, subprocess, sys, tomllib, os
here = os.path.dirname(os.path.abspath(__file__))
t = tomllib.load(open(os.path.join(here, "adapters.toml"), "rb"))
doc = {"format": t["adapters"]["format"], "artifacts": t["artifacts"]}
vec = json.load(open(os.path.join(here, "vectors.json")))
reqs = [{"doc": doc, "url": c["url"], **({"title": c["title"]} if "title" in c else {})} for c in vec["cases"]]
# also a title-less request for each case, to confirm validation alone
out = subprocess.run(sys.argv[1:], input="\n".join(json.dumps(r) for r in reqs) + "\n",
                     capture_output=True, text=True)
if out.returncode:
    print(out.stderr); sys.exit(2)
lines = [json.loads(l) for l in out.stdout.splitlines() if l.strip()]
fails = 0
for c, got in zip(vec["cases"], lines):
    if "error" in got:
        print("VALIDATION ERROR", got["error"]); sys.exit(3)
    e = c["expect"]; bad = []
    if got.get("match") != e["match"]: bad.append(f"match {got.get('match')}")
    if e["match"] and got.get("match"):
        if got["type"] != e["type"]: bad.append(f"type {got['type']}")
        if got["artifact_id"] != e["artifact_id"]: bad.append(f"id {got['artifact_id']}")
        if "title" in e and got.get("title") != e["title"]: bad.append(f"title {got.get('title')!r}")
    if bad:
        fails += 1; print("FAIL", c["id"], c["url"], bad, json.dumps(got)[:300])
pos = sum(c["expect"]["match"] for c in vec["cases"])
print(f"{len(lines)} cases ({pos} positive, {len(lines)-pos} negative): {len(lines)-fails} pass, {fails} fail")
sys.exit(1 if fails else 0)

#!/usr/bin/env python3
"""Throwaway validator for the Splunk blind starter (not product code).

Converts adapters.toml to the engine's document shape ({"format", "artifacts"}),
then runs every case in vectors.json through both of the repo's engines:
  - TypeScript: scripts/adapter-templates-eval.mjs (extension/dist build)
  - Rust: validate-rs (path dependency on adapter-templates), which also
    prints the host's overlap report (section 10.3)
and checks each result against the case's expectation and the two engines
against each other. Usage (from the worktree root, Node 24 and Rust on PATH):
  python3 -I research/blind/splunk-cloud/validate.py
"""
import json, os, subprocess, sys, tomllib

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
with open(os.path.join(HERE, "adapters.toml"), "rb") as f:
    toml = tomllib.load(f)
doc = {"format": toml["adapters"]["format"], "artifacts": toml["artifacts"]}
vectors = json.load(open(os.path.join(HERE, "vectors.json")))
doc_path = os.path.join(HERE, "validate-rs", "doc.json")
json.dump(doc, open(doc_path, "w"))

# TypeScript engine via the repo's Node harness.
lines = []
for c in vectors["cases"]:
    req = {"doc": doc, "url": c["url"]}
    if "title" in c:
        req["title"] = c["title"]
    lines.append(json.dumps(req))
ts = subprocess.run(["node", os.path.join(ROOT, "scripts", "adapter-templates-eval.mjs")],
                    input="\n".join(lines) + "\n", capture_output=True, text=True, cwd=ROOT)
if ts.returncode:
    sys.exit("node harness failed: " + ts.stderr)
ts_out = [json.loads(l) for l in ts.stdout.splitlines() if l.strip()]

# Rust engine via the throwaway checker.
rs = subprocess.run([os.path.join(HERE, "validate-rs", "target", "debug", "splunk-blind-check"),
                     doc_path, os.path.join(HERE, "vectors.json")], capture_output=True, text=True)
rs_lines = [json.loads(l) for l in rs.stdout.splitlines() if l.strip()]
header = [l for l in rs_lines if "case" not in l]
rs_out = [l["result"] for l in rs_lines if "case" in l]
for h in header:
    print("rust:", json.dumps(h))
if rs.returncode:
    sys.exit("rust checker failed: " + rs.stderr)

def norm(r):
    if not r.get("match"):
        return {"match": False}
    out = {"type": r["type"], "artifact_id": r["artifact_id"], "mode": r.get("mode")}
    if r.get("within"):
        out["within"] = r["within"]
    if r.get("title") is not None:
        out["title"] = r["title"]
    return out

failures = 0
counts = {}
for c, t, r in zip(vectors["cases"], ts_out, rs_out, strict=True):
    exp = c["expect"]
    got_t, got_r = norm(t), norm(r)
    ok_engines = got_t == got_r
    if exp.get("match") is False:
        ok = got_t == {"match": False}
    else:
        ok = all(got_t.get(k) == v for k, v in exp.items()) and ("within" in exp) == ("within" in got_t)
    counts.setdefault(c["group"], [0, 0])[0 if ok and ok_engines else 1] += 1
    if not (ok and ok_engines):
        failures += 1
        print("FAIL", c["group"], "-", c["name"], "\n  expected", exp, "\n  ts      ", t, "\n  rust    ", r)
for g, (p, f) in counts.items():
    print(f"{g}: {p} passed, {f} failed")
print("engines agree on every case" if all(norm(a) == norm(b) for a, b in zip(ts_out, rs_out)) else "ENGINES DISAGREE")
sys.exit(1 if failures else 0)

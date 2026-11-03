# Throwaway checker: TOML -> adapter doc JSON -> JSON Lines for an engine CLI
# (Node twin scripts/adapter-templates-eval.mjs, or the Rust checker), then
# compares results with vectors.json. Usage:
#   python3 -I check.py requests            > requests.jsonl
#   python3 -I check.py compare results.jsonl
import json, sys, tomllib, pathlib
here = pathlib.Path(__file__).parent
raw = tomllib.loads((here / "adapters.toml").read_text())
doc = {"format": raw["adapters"]["format"], "artifacts": raw["artifacts"]}
vec = json.loads((here / "vectors.json").read_text())["cases"]
if sys.argv[1] == "requests":
    print(json.dumps({"doc": doc, "url": "https://app.box.com/file/1"}))  # line 0: validation
    for c in vec:
        r = {"doc": doc, "url": c["url"]}
        if "title" in c: r["title"] = c["title"]
        print(json.dumps(r))
    sys.exit()
res = [json.loads(l) for l in open(sys.argv[2]) if l.strip()]
if "error" in res[0]:
    print("VALIDATION ERROR", res[0]); sys.exit(1)
print("document valid;", len(doc["artifacts"]), "adapters")
fails = 0
for c, r in zip(vec, res[1:]):
    e = c["expect"]; bad = []
    if r.get("match") != e["match"]: bad.append(f"match {r.get('match')} != {e['match']} ({r.get('reason')})")
    elif e["match"]:
        for k in ("type", "artifact_id", "mode"):
            if r.get(k) != e[k]: bad.append(f"{k} {r.get(k)!r} != {e[k]!r}")
        if "title" in e and r.get("title") != e["title"]: bad.append(f"title {r.get('title')!r} != {e['title']!r}")
    if bad: fails += 1; print("FAIL", c["id"], c["url"], "; ".join(bad), "| got", json.dumps(r))
print(f"{len(vec) - fails}/{len(vec)} vectors pass")
sys.exit(1 if fails else 0)

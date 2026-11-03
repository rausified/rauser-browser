#!/bin/sh
# Throwaway: validates adapters.toml and runs vectors.json through both engines.
# Run from anywhere; needs extension/dist built (npm run build:extension).
set -e
HERE=$(cd "$(dirname "$0")" && pwd); ROOT=$(cd "$HERE/../../.." && pwd); TMP=$(mktemp -d)
export PATH="$HOME/.nvm/versions/node/v24.14.0/bin:$HOME/.rustup/toolchains/1.98.1-aarch64-apple-darwin/bin:$PATH"
python3 -I "$HERE/check.py" requests > "$TMP/req.jsonl"
echo "== TypeScript engine (scripts/adapter-templates-eval.mjs)"
(cd "$ROOT" && node scripts/adapter-templates-eval.mjs < "$TMP/req.jsonl" 2>/dev/null > "$TMP/ts.jsonl")
python3 -I "$HERE/check.py" compare "$TMP/ts.jsonl"
echo "== Rust engine (adapter-templates crate via rust-check/)"
(cd "$HERE/rust-check" && cargo build --offline -q)
"$HERE/rust-check/target/debug/box-blind-check" < "$TMP/req.jsonl" > "$TMP/rs.jsonl"
python3 -I "$HERE/check.py" compare "$TMP/rs.jsonl"
echo "== overlap (Rust)"; head -1 "$TMP/req.jsonl" | OVERLAP=1 "$HERE/rust-check/target/debug/box-blind-check"
echo "== engines agree:"; python3 -I -c "
import json,sys
a=[json.loads(l) for l in open(sys.argv[1])]; b=[json.loads(l) for l in open(sys.argv[2])]
k=['match','type','artifact_id','mode','title','title_entry','rule','adapter','reason']
d=[i for i,(x,y) in enumerate(zip(a,b)) if any(x.get(j)!=y.get(j) for j in k)]
print('disagreements:', d or 'none'); sys.exit(1 if d else 0)" "$TMP/ts.jsonl" "$TMP/rs.jsonl"

#!/bin/sh
# Throwaway: validate adapters.toml and run vectors.json through both engines.
# Run from the worktree root: sh research/blind/google-workspace/validate.sh
set -e
G=research/blind/google-workspace
export PATH="$HOME/.nvm/versions/node/v24.14.0/bin:$HOME/.rustup/toolchains/1.98.1-aarch64-apple-darwin/bin:$PATH"
python3 $G/toml2json.py $G/adapters.toml $G/doc.json
node node_modules/typescript/bin/tsc --ignoreConfig extension/adapter-templates.ts --outDir $G/.build --target es2022 --module es2022 --lib es2024,dom --skipLibCheck
echo "== TypeScript engine (extension/adapter-templates.ts)"
node $G/run-vectors.mjs 2>/dev/null
echo "== Rust engine (adapter-templates crate)"
(cd $G/rust-check && cargo run --offline -q -- ..)

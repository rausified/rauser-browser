# Throwaway: converts adapters.toml to the engine's JSON document shape
# ({"format": 1, "artifacts": [...]}) for validate.mjs.
import json, sys, tomllib
t = tomllib.load(open(sys.argv[1], "rb"))
print(json.dumps({"format": t["adapters"]["format"], "artifacts": t["artifacts"]}))

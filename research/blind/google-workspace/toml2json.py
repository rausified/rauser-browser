# Throwaway: converts adapters.toml to the JSON document shape both engines accept.
import json, sys, tomllib
doc = tomllib.load(open(sys.argv[1], "rb"))
# The [adapters] table carries format; the engines take {format, artifacts}.
out = {"format": doc["adapters"]["format"], "artifacts": doc.get("artifacts", [])}
json.dump(out, open(sys.argv[2], "w"), indent=1)

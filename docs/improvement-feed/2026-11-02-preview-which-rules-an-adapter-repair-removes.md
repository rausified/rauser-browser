## 2026-11-02 — Preview which rules an adapter repair removes

Show a plain-language comparison of recognized adapters kept, changed, added
and removed before a complete-document repair is saved (`brwr-jxrw`, discovered
from `brwr-jnj`). The repair workspace protects the exact original with a backup
and shows preserved values, but manually rebuilding a document can still omit
useful supported rules. Compare supported-format values only; keep future
formats opaque and say when a comparison is unavailable. Add this review to
`extension/adapter-repair-settings.ts` and its offline settings tests, retaining
the revision gate and explicit replacement action.

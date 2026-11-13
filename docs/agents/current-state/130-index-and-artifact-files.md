# Index and artifact files

Built: a rebuildable SQLite `index.db` (cache format 9, written through `rusqlite` in small transactions and safe for several host processes at once), lazily regenerated artifact files, `brauser reindex`

Pointer: DESIGN.md §5.5, §6.7

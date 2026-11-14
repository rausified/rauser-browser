## 2026-11-10 — Show when the page editor uses a preserved note

Show a short indication when the editor uses an original note retained from an
earlier capture identity, and offer read-only access to preserved alternatives.
The ordinary-history integration in `brwr-8qgs` proves that a note can stay at
its original physical location while its public record key changes; canonical
preference also leaves other originals intact. The current panel does not explain
either situation. Add the host-derived presentation information to the note
response and render it beside the editor in `extension/panel.ts`, with explicit
viewing that never replaces or autosaves recovered or alternative text. This is
the remaining note-presentation work within `brwr-dbp`/`brwr-6x2`.
- Review 2026-11-12: **Revise.** Show that the editable note is stored under an earlier capture identity, so the user understands what is being edited. The requested read-only alternative-note view already exists.

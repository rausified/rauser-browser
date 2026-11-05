# Edit saved task commands in Settings

Bead: `brwr-kqt4`. Let a user create, edit and remove literal sidebar
commands without editing config.toml, and repair unsupported entries without
silently dropping their values.

## Open decisions

- Resolved by the bead and existing §5.6 contract: name, description and
  literal instruction fields; sidebar output only; explicit Save commands.
  Saving or listing a command never starts a task.
- Resolved by Cookie within the existing repair convention: invalid registries
  expose preserved commands-only TOML for explicit repair, with an exact config
  backup before replacement. Valid registries use labeled form rows.
- Resolved by the existing config contract: bind the complete config revision;
  stale saves keep the draft and refuse until current values are reviewed.

## Design challenge

Reviewer `/root/command_editor_design` proposed typed ordinary writes with
Rust serialization instead of client TOML generation. Keep one commands-only
TOML save because invalid-value repair already needs that input, but test JSON
string quoting against the real Rust TOML parser (quotes, backslashes,
newline/tab, Unicode and invalid surrogates). Use the existing registry parser.
The reviewer identified that snapshot serialization can discard unsupported
values: read the fresh bounded raw TOML table under the config lock, replace
only `commands`, cap the whole serialized config, publish with the existing
atomic helper and re-adopt disk state. Never clear unrelated issue states.
Refuse syntax-invalid or oversized files; recheck revision after backup.

## Delivery

1. Add narrow revision-bound native settings read/save messages and tests.
2. Add form editing and explicit invalid-TOML repair in Settings; retain drafts
   on config refresh, errors and stale saves. Reuse the existing mutation lock.
3. Verify native/wire/UI and browser accessibility cases, run the complete
   required/README gate, review the entire diff, document and hand off.

## Validation and completion

Implementation complete on 2026-11-05. Five native persistence/refusal tests,
one wire test, six Settings UI regressions and one browser keyboard/axe journey
cover the editor. Native tests include actual file publication, byte-preserving
repair backups, unrelated unsupported values, duplicates, templates, stale
revisions, whole-config bounds and literal Unicode/quote/backslash/newline/tab
strings. The fake-DOM suite exercises all authoring actions and stale drafts.

Baseline and the complete required/README gate passed on `6ea537e6` plus
this cycle: 1,626 workspace Rust-reported passes (94 explicit IPC guard skips, two ignored),
47 extension files, both Clippy configurations, 12 script files, 252 Python
tooling cases (92 platform skips), queue checks and 10,696 inert Markdown
renderings (the README corpus check reruns one Rust case, giving 1,627
reported passes across that combined gate). Accessibility reports two offline fixture passes and 49 explicit
sandbox browser skips; real browser/platform acceptance belongs to the driver
and CI. The browser fixture checks request/focus behavior; native tests prove
persistence.

Fresh complete manual diff review and one independent in-session Codex pass
found no authority or data-preservation defects. A final manual keyboard review
then identified browser focus loss while buttons were disabled; Save, stale
review and Close now restore focus after controls are enabled. Focus assertions
pass in the Settings tests. The required gate rerun for those final UI changes
passed; its output is in `target/command-editor/focus-final-gate.log`; the unchanged README extra
checks passed in `target/command-editor/final-gate.log`.

Ordinary ID freezing and the assigned Re-learn decision gates remain unchanged.
Follow-up `brwr-5wup` proposes showing the composed instruction budget before a
saved prompt reaches the existing task limit. Driver owns commit/publication
if Git metadata is read-only.

Closeout: brwr-kqt4 is closed after both gates passed. The single commit
attempt failed because `.git/index.lock` is read-only. No retry was made;
the complete green working tree and `.bosun_commit_msg` are ready for the
external driver. No remote Git or live service was used.

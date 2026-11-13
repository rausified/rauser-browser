## 2026-11-12 — Re-merge historical merge commits as real conflict fixtures

What to change: when building or changing merge tooling (the PR doctor, merge
drivers, pr-keeper's merge-tree checks), find real conflicts by re-merging the
two parents of past merge commits (`git merge-tree --write-tree --name-only
<M>^1 <M>^2`) and compare the tool's result with `<M>`, the resolution a
person committed. Keep a short list of such commits in the tool's tests or
TESTING.md.

Why: lane L2a's search for two open or merged PR heads that still conflict
found none, because every PR had already merged main in before it landed. But
re-merging the parents of `Merge origin/main into <branch>` commits gave real
generated-file conflicts at once (4ad210a9, bd182648: starter outputs only;
37ef5ec9: protocol types plus docs). The new `brauser-generated` driver's
output matched the committed hand resolution byte for byte. Synthetic
fixtures prove the mechanism; history shows the mechanism handles the
conflicts that actually happen.

Where: `scripts/tests/merge-generated.test.mjs`, `scripts/tests/pr-doctor.test.mjs`,
TESTING.md (a "merge tooling" note), and any future change to `scripts/pr-doctor.mjs`.

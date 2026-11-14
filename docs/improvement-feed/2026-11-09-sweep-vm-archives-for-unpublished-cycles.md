## 2026-11-09 — Sweep the worker VM's archive branches for unpublished cycles

Change: Add a coordinator check, run with the weekly queue review or by the driver after each archive, that lists commits on the Cookie VM's `archive/*` branches whose patch is not on `origin/main` (`git cherry origin/main <branch>`). For each one, it checks the feature against main. A real gap reopens or annotates its bead and gets a salvage PR. Docs-only health-gate commits are ignored.

Why: Four beads were closed or claimed on work that never reached main. Evidence cards (`39d78f887`, `brwr-edn4`), the Re-learn recorded-mentions fix (`12256be6d`, `brwr-d586`) and per-task working folders (`7f82e1ef7`/`fc4da28d6`, `brwr-zla`/`brwr-pf8o`) sat only on VM archive branches from Nov 3–4. Two dependent beads (`brwr-n4ga`, `brwr-fzya`) stayed blocked for six days, and `brwr-zla` showed as ready, inviting a second build. Three other blocked beads (`brwr-bzx`, `brwr-1xmv`, `brwr-9xg`) waited on "platform evidence" that main's CI already produced.

Where: `AGENTS.md` "Work queue" (the close-only-what-is-on-main and CI-as-platform-evidence rules added with this entry); the sweep itself belongs in the shared worker driver outside this repository.
- Review 2026-11-12: **Keep.** Compare worker archive changes against main and salvage genuine unpublished work; several closed or blocked beads referred to code only on VM branches.

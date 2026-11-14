## 2026-11-09 — Inject re-key filesystem sync errors

Add filesystem-error injection around prepare publication and recovery in
`host/src/relearn/transaction.rs` and `host/src/relearn/journal.rs`, including
directory-sync failures after a successful hard link. Independent review found
that a visible prepare record could survive a failed publication sync, while
transaction recovery proceeded without retrying that durability barrier. The
transaction now retries the sync with an injected-error regression. Extend
that coverage to the other filesystem operations and their ordering; the
existing post-operation interruption callbacks alone cannot prove that an
operation's own error leaves the correct recoverable state.

- +1 2026-11-09: Added injected history, config-parent and cleanup sync failures, including exact recovery after a private prepare reappears. Extend the same focused fixtures to file writes, renames and failed prepare hard-link publication; directory callback errors still cannot exercise those distinct failure states.
- Review 2026-11-12: **Keep.** Inject failures at remaining write, rename and publication steps; existing sync regressions cover some barriers but callbacks after operations cannot prove error recovery.

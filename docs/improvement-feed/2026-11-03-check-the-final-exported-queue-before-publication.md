## 2026-11-03 — Check the final exported queue before publication

Run `python3 scripts/check_queue_consistency.py` against the final exported
queue and backlog immediately before preparing the driver commit. Independent
review of `83c35391` found missing `brwr-9eio` and `brwr-fzya` rows even though
the backlog included them and the cycle recorded a green gate. Restoring the
exact rows from local commit `39d78f88` cleared all six reported violations.
Put the check after the last export or queue rewrite in the cycle closeout
path, so earlier successful checks cannot mask an incomplete final snapshot.

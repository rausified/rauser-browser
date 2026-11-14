## 2026-11-09 — Read test wall time before blaming compiles in CI

Change: Before speeding up a slow CI job, split each `cargo test` step into its compile and its run (the `Finished` line against the last `test result` line) and look for tests that log `has been running for over 60 seconds`. Treat a gap between platforms in the run part as an environment question (disk, temp folder, antivirus) before it is a caching one.

Why: The Windows build job took 998s and was put down to cold Rust compiles. Caching the compiled dependencies (`Swatinem/rust-cache`) only took it to 945s, because the two `cargo test` runs spent about 250s and 280s running tests, not compiling. Their SQLite tests sync after every commit and made their temporary folders on the runner's `C:` drive. Moving `TMP`/`TEMP` to the job's temp folder on `D:` cut those steps from 257s to 74s and from 327s to 99s with a warm cache (one run each; about 3.5x and 3.3x) and from 260s to 134s and 354s to 218s cold (about 2x), with the same 1992 tests passing. The whole job measured 422s warm, 766s cold, and 545s on a later real PR; state which configuration a multiplier came from rather than quoting one run's ratio as "the" speedup.

Where: `.github/workflows/build-os.yml` and the `.github/workflows/ci.yml` bullet in `AGENTS.md`.
- Review 2026-11-12: **Done as guidance.** Measure compile and test-run time separately before changing CI; Windows slowdown came mostly from temp-disk tests and was already improved.
- Done 2026-11-12: verified on main at 4cd617d; Windows temp-folder and cache work completed with measured run times.

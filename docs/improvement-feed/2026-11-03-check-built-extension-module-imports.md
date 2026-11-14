## 2026-11-03 — Check built extension module imports

Add a post-packaging check in `extension/build.mjs` that every relative module
import names an emitted file, with a fixture in the build tooling tests. During
`brwr-zla`, the TypeScript build passed while the explicit JavaScript copy list
omitted `task-folders.js`; composed UI tests then failed with
`ERR_MODULE_NOT_FOUND`. That omission is fixed in this change. A bundle check
would report this at build time for future features. Track as `brwr-m5dm`.
- Review 2026-11-12: **Keep.** Checks copied module imports after packaging so a TypeScript success cannot ship a missing JS file.

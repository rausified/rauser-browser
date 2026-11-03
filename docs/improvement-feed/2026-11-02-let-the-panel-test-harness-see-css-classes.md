## 2026-11-02 — Let the panel test harness see CSS classes

Change: have the fake DOM in `extension/tests/panel/harness.mjs` parse each
element's `class` attribute from the built `panel.html` and back `classList`
and `className` with a real set (`add`, `remove`, `toggle`, `contains`). Why:
hiding the page title and URL from the panel (they became `visually-hidden`)
could not be checked through `page.el(...)`, because `classList` there is a
no-op stub, so the regression test had to regex the raw `PANEL_HTML` string
instead. Any change that shows or hides something by class rather than the
`hidden` attribute has the same blind spot. Where: `harness.mjs` (`FakeElement`
and the id scan), then move the page-area check in
`extension/tests/panel/page-summary.test.mjs` onto it.

// [origin, expected StarterKind | null], computed from today's rules
// (secureHostname: canonical https origin only; a subdomain suffix match
// requires the bare suffix host itself to be excluded). Shared by
// extension/tests/starter-families.test.mjs (starterKind, the engine's own
// best-match lookup) and scripts/tests/generate-starters.test.mjs
// (familyClaims, the generator's per-family check): both must agree on
// every case, or a family file could validate while the shipped matcher
// disagrees with it at runtime.
export const ORIGIN_CASES = [
  // Atlassian (.atlassian.net subdomain)
  ["https://acme.atlassian.net", "atlassian"],
  ["https://atlassian.net", null], // the bare suffix host itself is excluded
  ["http://acme.atlassian.net", null], // not https
  // A non-default port is accepted today: secureHostname compares `.hostname`,
  // which drops the port, so the suffix check still matches.
  ["https://acme.atlassian.net:8443", "atlassian"],
  ["https://ACME.atlassian.net", null], // not a canonical origin (URL lowercases the host)
  ["https://acme.atlassian.net/", null], // trailing slash is not a canonical origin
  ["https://evil-atlassian.net", null], // no dot before "atlassian.net"
  ["https://a.b.atlassian.net", "atlassian"], // nested subdomain still matches
  ["https://acme.atlassian.net.evil.example", null], // suffix only, not a match
  ["https://acme.atlassian.net?x=1", null], // query string is not a canonical origin
  ["https://x.atlassian.net", "atlassian"], // minimal one-letter subdomain
  ["https://acme.ATLASSIAN.net", null], // not a canonical origin

  // SharePoint (.sharepoint.com subdomain)
  ["https://acme-my.sharepoint.com", "sharepoint"],
  ["https://sharepoint.com", null],
  ["http://acme.sharepoint.com", null],
  ["https://acme.sharepoint.com.evil.example", null],
  ["https://evil-sharepoint.com", null],
  ["https://a.b.sharepoint.com", "sharepoint"],
  ["https://ACME.sharepoint.com", null],
  ["https://acme.sharepoint.com:443", null], // the default port is stripped, so this is not canonical
  ["https://acme.sharepoint.com/", null],
  ["https://my.sharepoint.com", "sharepoint"], // "my" is just another subdomain label here

  // GitHub (exact origin)
  ["https://github.com", "github"],
  ["https://www.github.com", null],
  ["http://github.com", null],
  ["https://github.com.evil.example", null],
  ["https://github.com/", null],
  ["https://api.github.com", null],
  ["https://GITHUB.com", null],
  ["https://github.com:443", null],
  ["https://github.co", null],

  // Miro (exact origin)
  ["https://miro.com", "miro"],
  ["https://www.miro.com", null],
  ["http://miro.com", null],
  ["https://miro.com.evil.example", null],
  ["https://miro.com/", null],
  ["https://app.miro.com", null],
  ["https://MIRO.com", null],
  ["https://miro.com:443", null],
  ["https://miro.co", null],
];

// A strict subdomain family (labels: 1, exclude: ["www", "signin"]),
// test-only, exercising Task 4's tighter subdomain rule: unlike Atlassian
// and SharePoint above, which claim any subdomain prefix (including nested
// ones and "www"/"signin"-style product hosts), a strict family claims only
// a single label that is not on its exclude list. Shared the same way as
// ORIGIN_CASES, between extension/tests/starter-families.test.mjs
// (starterKind, with EXAMPLE_STRICT_FAMILY added to STARTER_FAMILIES via
// setStarterFamiliesForTest) and scripts/tests/generate-starters.test.mjs
// (familyClaims applied directly to EXAMPLE_STRICT_FAMILY.origin).
export const STRICT_ORIGIN_CASES = [
  ["https://acme.strict.test", "strict"],
  ["https://www.strict.test", null], // excluded product host
  ["https://signin.strict.test", null], // excluded product host
  ["https://a.b.strict.test", null], // two labels: labels: 1 refuses it
  ["https://strict.test", null], // the bare suffix host itself is excluded
  ["https://acme.strict.test:8443", "strict"], // same port rule as Atlassian's sweep row
  ["https://acme.strict.test.", null], // trailing dot is not a canonical origin
  ["http://acme.strict.test", null], // not https
  ["https://ACME.strict.test", null], // not a canonical origin (URL lowercases the host)
  ["https://acme-www.strict.test", "strict"], // one label; "acme-www" is not the excluded "www"
];

// The strict shipped families' own origin edges (box, webex, aha and the
// Aha! ideas portal family aha-ideas): prefix excludes (`"fupload-*"`), a
// multi-origin family (Box: personal `https://app.box.com`,
// `<company>.app.box.com` and `<company>.ent.box.com`), and a `labels: 1`
// family coexisting with a deeper suffix (`.aha.io` and `.ideas.aha.io`).
// Shared the same way as ORIGIN_CASES: extension/tests/starter-families.test.mjs
// checks starterKind against the shipped STARTER_FAMILIES, and
// scripts/tests/generate-starters.test.mjs checks that exactly the expected
// family's familyClaims holds.
export const SHIPPED_STRICT_ORIGIN_CASES = [
  // Box: three origins, one family
  ["https://acme.app.box.com", "box"],
  ["https://app.box.com", "box"], // personal Box: the exact origin member
  ["https://acme.ent.box.com", "box"], // Box Zones enterprise host
  ["https://fupload-12.app.box.com", null], // upload host: the "fupload-*" prefix exclude
  ["https://fupload-1.ent.box.com", null], // same prefix exclude on the enterprise member
  ["https://fupload.app.box.com", "box"], // "fupload" alone does not start with "fupload-"
  ["https://acme-fupload-1.app.box.com", "box"], // prefix, not substring
  ["https://www.app.box.com", null],
  ["https://cloud.app.box.com", null],
  ["https://a.b.app.box.com", null], // labels: 1
  ["https://ent.box.com", null], // the bare suffix host itself
  ["https://www.box.com", null], // marketing host, outside every member
  ["https://acme.box.com", null], // redirects to acme.app.box.com; not claimed
  ["https://app.box.com:8443", null], // an exact member compares the whole origin

  // Webex: the media host the research names
  ["https://acme.webex.com", "webex"],
  ["https://nsj1wss.webex.com", null],
  ["https://www.webex.com", null],

  // Aha!: the app family and the ideas portal family side by side
  ["https://acme.aha.io", "aha"],
  ["https://acme.ideas.aha.io", "aha-ideas"],
  ["https://ideas.aha.io", null], // excluded by aha, the bare suffix host for aha-ideas
  ["https://a.b.ideas.aha.io", null], // labels: 1 on the portal family too
  ["https://acme.identity.aha.io", null], // portal SSO: two labels for aha
];

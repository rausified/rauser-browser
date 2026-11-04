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

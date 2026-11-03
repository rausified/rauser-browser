// Unit tests for the STARTER_FAMILIES-driven starterKind/starterStatus in
// extension/starter-adapters.ts, from dist (npm run build:extension).
import assert from "node:assert/strict";
import test from "node:test";

const { starterKind, starterStatus } = await import("../dist/starter-adapters.js");

// [origin, expected StarterKind | null], computed from today's rules
// (secureHostname: canonical https origin only; a subdomain suffix match
// requires the bare suffix host itself to be excluded).
const ORIGIN_CASES = [
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

test("starterKind agrees with the legacy rules on 40 origins", () => {
  assert.equal(ORIGIN_CASES.length, 40);
  for (const [origin, expected] of ORIGIN_CASES) {
    assert.equal(starterKind(origin), expected, origin);
  }
});

test("every family type exposes its status", () => {
  assert.equal(starterStatus("jira.issue")?.state, "certified");
  assert.equal(starterStatus("confluence.page")?.state, "development");
  assert.equal(starterStatus("nope"), null);
});

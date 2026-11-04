// Unit tests for the STARTER_FAMILIES-driven starterKind/starterStatus in
// extension/starter-adapters.ts, from dist (npm run build:extension).
import assert from "node:assert/strict";
import test from "node:test";

const {
  starterKind,
  starterStatus,
  starterOffers,
  starterFamily,
  displayTypes,
  recordLabel,
  recordOrder,
  isContainerType,
  configuredScope,
  matchingStarter,
  starterRuleAddition,
  setStarterFamiliesForTest,
} = await import("../dist/starter-adapters.js");
const { EXAMPLE_TENANT_FAMILY } = await import("./starter-fixtures.mjs");
const { ORIGIN_CASES } = await import("./starter-origin-cases.mjs");

// A tenant_name family whose one type has two match rules (EXAMPLE_TENANT_FAMILY's
// example.item has only one), test-only, to exercise starterRuleAddition's
// "a saved adapter is missing one of the current starter's rules" path on a
// family keyed by something other than GitHub's "owner".
const TENANT_TWO_RULE_FAMILY = {
  ...EXAMPLE_TENANT_FAMILY,
  types: [
    {
      ...EXAMPLE_TENANT_FAMILY.types[0],
      adapter: {
        ...EXAMPLE_TENANT_FAMILY.types[0].adapter,
        match: [
          ...EXAMPLE_TENANT_FAMILY.types[0].adapter.match,
          { path: "/{workspace:name|lower}/item/{id:int}/detail" },
        ],
      },
    },
  ],
};

test("starterKind agrees with the legacy rules on 40 origins", () => {
  assert.equal(ORIGIN_CASES.length, 40);
  for (const [origin, expected] of ORIGIN_CASES) {
    assert.equal(starterKind(origin), expected, origin);
  }
});

test("an origin containing a $ replacement pattern does not corrupt the offer key", () => {
  // String.replace treats "$'" in a replacement string as "everything after
  // the match"; a function replacer (extension/starter-adapters.ts
  // starterOffers) must not fall into that trap just because the URL parser
  // accepts "$" in a hostname.
  const origin = "https://a$'.atlassian.net";
  const offers = starterOffers(origin, []);
  const jiraOffer = offers.find((offer) => offer.artifact_type === "jira.issue");
  assert.ok(jiraOffer, "expected a jira.issue offer for an atlassian origin");
  assert.equal(jiraOffer.key, `jira:${origin}`);
});

test("every family type exposes its status", () => {
  assert.equal(starterStatus("jira.issue")?.state, "certified");
  assert.equal(starterStatus("confluence.page")?.state, "development");
  assert.equal(starterStatus("nope"), null);
});

test("displayTypes puts containers first", () => {
  const github = starterFamily("https://github.com");
  assert.deepEqual(displayTypes(github).map((type) => type.type), ["github.repo", "github.pr", "github.issue"]);
  const atlassian = starterFamily("https://acme.atlassian.net");
  assert.deepEqual(displayTypes(atlassian).map((type) => type.type), ["jira.issue", "confluence.page"]);
});

test("record labels and order come from the family files", () => {
  assert.equal(recordLabel("github.pr"), "pull requests");
  assert.equal(recordLabel("jira.issue"), null);
  assert.deepEqual(recordOrder(), ["github.repo", "github.pr", "github.issue"]);
});

test("isContainerType", () => {
  assert.equal(isContainerType("github.repo"), true);
  assert.equal(isContainerType("github.pr"), false);
  assert.equal(isContainerType("jira.issue"), false);
});

test("configuredScope reads every scope value on the origin", () => {
  const artifacts = [
    { type: "github.repo", origin: "https://github.com", scope: { owner: ["A"] } },
    { type: "github.repo", origin: "https://github.com", scope: { owner: ["b", "a"] } },
    { type: "jira.issue", origin: "https://acme.atlassian.net", scope: { owner: ["ignored"] } },
  ];
  assert.deepEqual(configuredScope(artifacts, "https://github.com"), ["a", "b"]);
});

test("configuredScope reads only the scope key the family's templates mark, not every key on the adapter", () => {
  // github.json's adapters mark only "owner" with {"$scope": true}; a
  // hand-written GitHub adapter scoped by a second capture (for example a
  // user who narrowed the match to one repository) must not leak that
  // "repo" key into the gallery's organization field as if it were an
  // organization the starter itself recorded.
  const artifacts = [
    { type: "github.repo", origin: "https://github.com", scope: { owner: ["acme"], repo: ["api"] } },
  ];
  assert.deepEqual(configuredScope(artifacts, "https://github.com"), ["acme"]);
});

test("matchingStarter recognises a saved tenant-scoped adapter by its family's own marked scope key, not GitHub's owner", () => {
  // Codex P2: matchingStarter used to pass ownersOf(adapter) (which only
  // ever reads scope.owner) into starterOffers, so a saved tenant_name
  // starter's scope was read as empty, starterOffers refused to build an
  // adapter for a scoped type with no values, and the saved adapter could
  // never be found among the (all-null) offers.
  setStarterFamiliesForTest([EXAMPLE_TENANT_FAMILY]);
  try {
    const saved = {
      type: "example.item",
      origin: "https://example.test",
      scope: { workspace: ["acme"] },
      match: [{ path: "/{workspace:name|lower}/item/{id:int}" }],
      id: "example.item:{host}/{workspace}/{id}",
    };
    const offer = matchingStarter(saved);
    assert.ok(offer, "expected the tenant-scoped adapter to match its starter");
    assert.equal(offer.artifact_type, "example.item");
  } finally {
    setStarterFamiliesForTest(null);
  }
});

test("starterRuleAddition offers a missing rule on a saved tenant-scoped adapter", () => {
  setStarterFamiliesForTest([TENANT_TWO_RULE_FAMILY]);
  try {
    const detailRule = TENANT_TWO_RULE_FAMILY.types[0].adapter.match[1];
    const saved = {
      type: "example.item",
      origin: "https://example.test",
      scope: { workspace: ["acme"] },
      match: [TENANT_TWO_RULE_FAMILY.types[0].adapter.match[0]],
      id: "example.item:{host}/{workspace}/{id}",
    };
    const update = starterRuleAddition(saved, detailRule);
    assert.ok(update, "expected a rule addition for the tenant-scoped adapter");
    assert.deepEqual(update.added, [detailRule]);
  } finally {
    setStarterFamiliesForTest(null);
  }
});

test("the test seam swaps and restores the family list", () => {
  setStarterFamiliesForTest([EXAMPLE_TENANT_FAMILY]);
  try {
    assert.equal(starterKind("https://example.test"), "example");
    assert.equal(starterKind("https://github.com"), null);
    const offers = starterOffers("https://example.test", ["acme"]);
    assert.deepEqual(offers[0].adapter.scope, { workspace: ["acme"] });
  } finally {
    setStarterFamiliesForTest(null);
  }
  assert.equal(starterKind("https://example.test"), null);
  assert.equal(starterKind("https://github.com"), "github");
});

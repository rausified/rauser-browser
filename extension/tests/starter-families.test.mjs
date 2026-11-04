// Unit tests for the STARTER_FAMILIES-driven starterKind/starterStatus in
// extension/starter-adapters.ts, from dist (npm run build:extension).
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
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
const { compileDocument, evaluate } = await import("../dist/adapter-templates.js");
const { EXAMPLE_TENANT_FAMILY, EXAMPLE_STRICT_FAMILY } = await import("./starter-fixtures.mjs");
const { ORIGIN_CASES, STRICT_ORIGIN_CASES } = await import("./starter-origin-cases.mjs");
const { loadFamilies } = await import("../../scripts/generate-starters.mjs");
const { STARTER_FAMILIES } = await import("../dist/starter-families.generated.js");

const REPO = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");

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

test("strict subdomain families claim one label and skip excluded hosts", (t) => {
  setStarterFamiliesForTest([...STARTER_FAMILIES, EXAMPLE_STRICT_FAMILY]);
  t.after(() => setStarterFamiliesForTest(null));
  for (const [origin, expected] of STRICT_ORIGIN_CASES) {
    assert.equal(starterKind(origin), expected, origin);
  }
  // Adding the strict family must not change what the real families claim.
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

/** The scope value a titled, scoped sample's own URL needs for
 * `starterOffers` to build a matching adapter: today, only `github_org`
 * families have titled samples, and GitHub's path-captured `owner` is
 * lower-cased (`{owner:name|lower}`), so the org list must hold the
 * lower-cased first path segment. */
function scopeOrganizationsFor(family, url) {
  if (family.scope?.kind !== "github_org") return [];
  const owner = new URL(url).pathname.split("/").filter(Boolean)[0] ?? "";
  return [owner.toLowerCase()];
}

test("every titled sample's recorded title agrees with its own URL's captures (adapter-templates.md §9.1)", () => {
  // protocol/starters/*.json's `samples` (and their `title`) are test-only:
  // renderTs strips them from STARTER_FAMILIES, and renderStarterShapes
  // keeps only {note, url, shape} in starter-shapes.json. Load the family
  // files directly, build each sample's real adapter through starterOffers
  // (the shipped STARTER_FAMILIES, scoped from the sample's own URL when its
  // family needs scope), and check the engine actually matches the sample's
  // title to a template entry. A template whose same-named capture (for
  // example `{key}`, `{owner}`, `{number}`) disagrees with the URL's own
  // capture does not match at all (§9.1), so `title_entry !== null` is
  // exactly the check: a recorded sample whose title names a different key,
  // owner/repo or number than its own URL would fail this.
  const families = loadFamilies(REPO);
  let titledSamples = 0;
  for (const family of families) {
    for (const type of family.types) {
      for (const sample of type.samples) {
        if (sample.title === null) continue;
        titledSamples += 1;
        const origin = new URL(sample.url).origin;
        const organizations = scopeOrganizationsFor(family, sample.url);
        const offer = starterOffers(origin, organizations).find((o) => o.artifact_type === type.type);
        assert.ok(offer, `${type.type}: no starter offer for ${origin}`);
        assert.ok(offer.adapter, `${type.type}: ${sample.note} needs a scope value to build its adapter`);
        const compiled = compileDocument({ format: 1, artifacts: [offer.adapter] });
        assert.ok(compiled.ok, `${type.type}: ${sample.note}'s adapter compiles: ${JSON.stringify(compiled.error)}`);
        const result = evaluate(compiled.doc, sample.url, sample.title);
        assert.equal(result.match, true, `${sample.note}: ${sample.url} must match its own type's adapter`);
        assert.notEqual(
          result.title_entry,
          null,
          `${sample.note}: title "${sample.title}" must match a title template entry for ${sample.url}`,
        );
      }
    }
  }
  assert.ok(titledSamples >= 5, `expected at least 5 titled samples, saw ${titledSamples}`);
});

test("starter-vectors fixtures are the runtime offers", () => {
  // protocol/adapter-templates/starter-vectors.json's starter:<family>
  // fixtures (scripts/generate-starters.mjs synthesizeFixture) must stay
  // exactly the documents starterOffers builds for the family's own
  // sample_origin and sample_scope at runtime: the load-bearing guarantee
  // that a generated fixture is not a stand-in the engines test against
  // while the shipped offers quietly drift from it.
  const families = loadFamilies(REPO);
  const starterVectors = JSON.parse(
    readFileSync(path.join(REPO, "protocol", "adapter-templates", "starter-vectors.json"), "utf8"),
  );
  assert.ok(families.length > 0);
  for (const family of families) {
    const fixture = starterVectors.fixtures[`starter:${family.family}`];
    assert.ok(fixture, `starter-vectors.json has no starter:${family.family} fixture`);
    const offers = starterOffers(family.sample_origin, family.sample_scope);
    const adapters = family.types.map((type) => {
      const offer = offers.find((candidate) => candidate.artifact_type === type.type);
      assert.ok(offer, `${family.sample_origin} offers no ${type.type}`);
      return offer.adapter;
    });
    assert.deepEqual(fixture.artifacts, adapters, family.family);
  }
});

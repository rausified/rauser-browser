// Synthetic starter families for tests that need a tenant_name or unscoped
// family without depending on the shipped catalog (protocol/starters/*.json).
// Shaped exactly like a generated StarterFamilyData (extension/
// starter-families.generated.ts) so they can be passed to
// setStarterFamiliesForTest in extension/starter-adapters.ts.

export const EXAMPLE_TENANT_FAMILY = {
  family: "example",
  display_name: "Example",
  review_prompt: "Choose what to remember on Example.",
  origin: { kind: "exact", origin: "https://example.test" },
  scope: { kind: "tenant_name", max: 4, label: "Workspaces", placeholder: "acme" },
  types: [
    {
      type: "example.item",
      key: "example.item:{host}/{workspace}/{id}",
      label: "Example items",
      label_variants: [],
      description: "An example item.",
      record_label: null,
      example_path: null,
      adapter: {
        type: "example.item",
        match: [{ path: "/{workspace:name|lower}/item/{id:int}" }],
        scope: { workspace: { "$scope": true } },
        id: "example.item:{host}/{workspace}/{id}",
      },
      history: [],
      status: { state: "development", method: null, checked_on: null, checked_by: null },
      docs: "docs/adapters/example.item.md",
    },
    {
      type: "example.note",
      key: "example.note:{host}/{workspace}/{id}",
      label: "Example notes",
      label_variants: [],
      description: "An example note.",
      record_label: null,
      example_path: null,
      adapter: {
        type: "example.note",
        match: [{ path: "/{workspace:name|lower}/note/{id:int}" }],
        scope: { workspace: { "$scope": true } },
        id: "example.note:{host}/{workspace}/{id}",
      },
      history: [],
      status: { state: "development", method: null, checked_on: null, checked_by: null },
      docs: "docs/adapters/example.note.md",
    },
  ],
};

export const EXAMPLE_UNSCOPED_FAMILY = {
  family: "everyone",
  display_name: "Everyone",
  review_prompt: "Choose whether to remember Everyone pages.",
  // The real adapter-templates engine (extension/adapter-templates.ts
  // `SHARED_ORIGINS`) only allows an `unscoped` marker on its five hardcoded
  // shared origins; an unscoped family's fixture has to use one of them, not
  // a synthetic test domain, or a full plan/validate round trip fails
  // "unscoped_not_shared". Figma is in that set and claimed by no starter
  // family or other fixture.
  origin: { kind: "exact", origin: "https://www.figma.com" },
  scope: {
    kind: "unscoped",
    notice: "Records every Everyone page from every account in this Chrome profile.",
  },
  types: [
    {
      type: "everyone.page",
      key: "everyone.page:{host}/{id}",
      label: "Everyone pages",
      label_variants: [],
      description: "An example unscoped page.",
      record_label: null,
      example_path: null,
      adapter: {
        type: "everyone.page",
        match: [{ path: "/p/{id:int}" }],
        unscoped: "acknowledged",
        id: "everyone.page:{host}/{id}",
      },
      history: [],
      status: { state: "development", method: null, checked_on: null, checked_by: null },
      docs: "docs/adapters/everyone.page.md",
    },
  ],
};

// A two-type unscoped family, for tests that need to tell "the notice shows
// once" (EXAMPLE_UNSCOPED_FAMILY, one type) apart from "the notice shows
// before every checkbox and on every gallery card" (this fixture). Notion is
// in SHARED_ORIGINS and claimed by no starter family or other fixture.
export const EXAMPLE_UNSCOPED_PAIR_FAMILY = {
  family: "pair",
  display_name: "Pair",
  review_prompt: "Choose what to remember on Pair.",
  origin: { kind: "exact", origin: "https://www.notion.so" },
  scope: {
    kind: "unscoped",
    notice: "Records every Pair page and board from every account in this Chrome profile.",
  },
  types: [
    {
      type: "pair.page",
      key: "pair.page:{host}/{id}",
      label: "Pair pages",
      label_variants: [],
      description: "An example unscoped page.",
      record_label: null,
      example_path: null,
      adapter: {
        type: "pair.page",
        match: [{ path: "/p/{id:int}" }],
        unscoped: "acknowledged",
        id: "pair.page:{host}/{id}",
      },
      history: [],
      status: { state: "development", method: null, checked_on: null, checked_by: null },
      docs: "docs/adapters/pair.page.md",
    },
    {
      type: "pair.board",
      key: "pair.board:{host}/{id}",
      label: "Pair boards",
      label_variants: [],
      description: "An example unscoped board.",
      record_label: null,
      example_path: null,
      adapter: {
        type: "pair.board",
        match: [{ path: "/b/{id:int}" }],
        unscoped: "acknowledged",
        id: "pair.board:{host}/{id}",
      },
      history: [],
      status: { state: "development", method: null, checked_on: null, checked_by: null },
      docs: "docs/adapters/pair.board.md",
    },
  ],
};

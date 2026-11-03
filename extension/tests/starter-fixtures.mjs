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
  review_prompt: null,
  origin: { kind: "exact", origin: "https://everyone.test" },
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

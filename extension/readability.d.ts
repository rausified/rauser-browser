// The global that dist/vendor/readability.js (pinned @mozilla/readability)
// defines in the page's isolated world before extractReadablePage runs.
declare class Readability {
  constructor(doc: Node, options?: { serializer?: (el: Element) => unknown });
  parse(): { title: string; content: unknown } | null;
}

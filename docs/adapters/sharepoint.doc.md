# SharePoint and OneDrive for work document starter draft

Status: development draft. The adapter grammar and recorded examples were
verified by both adapter-template engines on 2026-10-16. A live SharePoint and
OneDrive UI check is still required before this draft is release-certified.

Brauser offers this draft on exact HTTPS `*.sharepoint.com` origins, including
OneDrive for work tenants served from `<tenant>-my.sharepoint.com`. It covers
the common Office document routes under `/sites/` and `/personal/`, including
the `/:w:/r/` sharing form. Route literals compare without ASCII case, and the
braced or unbraced `sourcedoc` UUID becomes the identity:

`sharepoint:<exact host>/<lowercase UUID>`

The draft deliberately does not recognize folder listings, search results,
ordinary site pages, or document links that carry no `sourcedoc` UUID. It uses
the whole tab title because the recorded format does not yet justify a stable
title template. A tenant that emits a different link shape should use the
match preview to add only that reviewed route.

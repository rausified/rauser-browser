# SharePoint and OneDrive for work document starter draft

Status: release-certified 2026-11-02. John checked this draft against the live
SharePoint and OneDrive UI on macOS with real Chrome on 2026-11-02, after both
adapter-template engines verified its grammar and recorded examples on
2026-10-28.

Brauser offers this draft on exact HTTPS `*.sharepoint.com` origins, including
OneDrive for work tenants served from `<tenant>-my.sharepoint.com`. It covers
the common Office document routes under `/sites/` and `/personal/`, plus the
Office sharing-link form that OneDrive and SharePoint use when a document is
shared with you (`/:w:/`, `/:x:/`, or `/:p:/`, then `/r/` or `/g/`, then
`/sites/` or `/personal/`). Route literals compare without ASCII case, and the
braced or unbraced `sourcedoc` UUID becomes the identity:

`sharepoint:<exact host>/<lowercase UUID>`

Recorded samples, one per rule:

- `https://acme.sharepoint.com/sites/Eng/_layouts/15/Doc.aspx?sourcedoc=%7B0A1B2C3D-4E5F-6A7B-8C9D-0E1F2A3B4C5D%7D&file=Plan.docx&action=default&mobileredirect=true`
  — a site document opened directly.
- `https://acme.sharepoint.com/:w:/r/sites/Eng/_layouts/15/Doc.aspx?sourcedoc=%7B0A1B2C3D-4E5F-6A7B-8C9D-0E1F2A3B4C5D%7D&file=Plan.docx&action=default&mobileredirect=true`
  — a Word document shared from a site.
- `https://acme-my.sharepoint.com/:w:/r/personal/jdoe_acme_com/_layouts/15/Doc.aspx?sourcedoc=%7B0A1B2C3D-4E5F-6A7B-8C9D-0E1F2A3B4C5D%7D&file=Plan.docx&action=default&mobileredirect=true`
  — a Word document shared from OneDrive for work, the form a recipient sees
  when another user shares a document with them (`&file=…&action=default&`
  and similar tracking parameters, such as `mobileredirect` or `wdpwf`, vary
  and are ignored).
- `https://acme-my.sharepoint.com/:x:/r/personal/jdoe_acme_com/_layouts/15/Doc.aspx?sourcedoc=%7B0A1B2C3D-4E5F-6A7B-8C9D-0E1F2A3B4C5D%7D&file=Budget.xlsx&action=default&mobileredirect=true`
  — the Excel equivalent (`:x:`).
- `https://acme.sharepoint.com/:p:/g/sites/Eng/_layouts/15/Doc.aspx?sourcedoc=%7B0A1B2C3D-4E5F-6A7B-8C9D-0E1F2A3B4C5D%7D&file=Deck.pptx&action=default&mobileredirect=true`
  — the PowerPoint equivalent (`:p:`), shared with the `/g/` guest-link form
  on a site.
- `https://acme-my.sharepoint.com/personal/jdoe_acme_com/_layouts/15/Doc.aspx?sourcedoc=%7B0A1B2C3D-4E5F-6A7B-8C9D-0E1F2A3B4C5D%7D&file=Plan.docx&action=default&mobileredirect=true`
  — a personal (OneDrive for work) document opened directly.

Separately from this draft, the Brauser service recognises these Office
document routes (and the plain `:w:`/`:x:`/`:p:` sharing links) from a page's
URL so that a task or summary run's agent can fetch the document itself
(DESIGN.md §4.5 "Office documents"), whether or not this starter is adopted;
that never changes record keys.

The draft deliberately does not recognize folder listings, search results,
ordinary site pages, a sharing link with no `sourcedoc` UUID (such as
`https://acme-my.sharepoint.com/:w:/r/personal/x/_layouts/15/Doc.aspx`), or a
document link that is not the `_layouts/15/Doc.aspx` sharing/open redirect at
all (such as `https://acme-my.sharepoint.com/:w:/r/personal/x/Documents/a.docx`).
It also does not recognize the short guest-link form that has no
`_layouts/15/Doc.aspx` segment and carries no `sourcedoc`, such as
`https://acme-my.sharepoint.com/:x:/g/personal/jdoe_acme_com/EbCdEfGh`; that
link only resolves to a document after a redirect Brauser does not follow.

`generic_titles = ["Working...", "Working…", "Login", "Redirecting", "Error",
"Sign in is not complete"]` lists the titles shown by SharePoint's loading,
redirect, sign-in and error pages instead of the item's own name
(`adapter-templates.md` §9.4). A visit whose title is one of them, ASCII case
aside, or is empty or the bare host, records no title, so the artifact keeps
the title it already has ("Untitled" for a new one).

Known limits: sharing links for other kinds of file are not recognized, even
in the `_layouts/15/Doc.aspx` form, because only Word, Excel and PowerPoint
are recorded: `/:b:/` (PDF), `/:o:/` (OneNote), `/:v:/` (video), and `/:u:/`
(other files). Neither is the older `/:w:/s/` sharing form. Each needs its own
recorded sample before it joins the starter.

It uses the whole tab title because the recorded format does not yet justify a
stable title template. A tenant that emits a different link shape should use
the match preview to add only that reviewed route.

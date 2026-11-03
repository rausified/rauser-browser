# Throwaway: writes vectors.json (synthetic IDs only). Run: python3 -I gen_vectors.py
import json, pathlib
A, E = "https://app.box.com", "https://acme.app.box.com"
P = " | Powered by Box"
def pos(id, url, type_, aid, title=None, want=None, mode=None, why=""):
    c = {"id": id, "why": why, "url": url, "expect": {"match": True, "type": type_, "artifact_id": aid, "mode": mode}}
    if title is not None:
        c["title"] = title; c["expect"]["title"] = want
    return c
def neg(id, url, why):
    return {"id": id, "why": why, "url": url, "expect": {"match": False}}
F, D, S = "box.file", "box.folder", "box.share"
cases = [
 # box.file
 pos("file.preview", f"{A}/file/111122223333", F, "box.file:app.box.com/111122223333", "Q3 Plan.pptx"+P, "Q3 Plan.pptx", why="logged-in preview"),
 pos("file.preview.slash_query_fragment", f"{A}/file/111122223333/?s=abcdefghij0123456789#page=2", F, "box.file:app.box.com/111122223333", why="trailing slash, ?s= share token and fragment ignored"),
 pos("file.enterprise", f"{E}/file/444455556666?sb=/details", F, "box.file:acme.app.box.com/444455556666", "Budget FY27.xlsx"+P, "Budget FY27.xlsx", why="custom subdomain; host in ID"),
 pos("file.note", f"{A}/notes/222233334444", F, "box.file:app.box.com/222233334444", "Weekly sync.boxnote"+P, "Weekly sync.boxnote", why="Box Note: same file-ID namespace (title form unverified)"),
 pos("file.note.query", f"{A}/notes/222233334444?s=zzzzyyyyxxxxwwwwvvvv", F, "box.file:app.box.com/222233334444", why="note opened with share query"),
 pos("file.canvas", f"{A}/canvas/333344445555", F, "box.file:app.box.com/333344445555", "Roadmap board.boxcanvas"+P, "Roadmap board.boxcanvas", why="Box Canvas route (title unverified)"),
 pos("file.in_shared_folder", f"{A}/s/abcd1234efgh5678ijkl9012mnop3456/file/555566667777", F, "box.file:app.box.com/555566667777", "Press kit.pdf"+P, "Press kit.pdf", why="file inside a shared-link folder, observed form"),
 pos("file.in_vanity_folder", f"{E}/v/press-kit/file/666677778888?sortColumn=name&sortDirection=ASC", F, "box.file:acme.app.box.com/666677778888", "Logo.png"+P, "Logo.png", why="file inside a custom-URL folder"),
 pos("file.title_with_pipe", f"{A}/file/777788889999", F, "box.file:app.box.com/777788889999", "A | B notes.docx"+P, "A | B notes.docx", why="title containing ' | ' kept whole"),
 pos("file.officeonline", f"{A}/integrations/officeonline/openOfficeOnline?fileId=888899990000&sharedAccessCode=", F, "box.file:app.box.com/888899990000", mode="edit", why="LOW confidence route: Office for the web editing"),
 pos("file.default_title", f"{A}/file/999900001111", F, "box.file:app.box.com/999900001111", "Box | Simple Online Collaboration", "Box | Simple Online Collaboration", why="default title matches no entry: Brauser keeps the whole tab title (title_entry null)"),
 # box.folder
 pos("folder.view", f"{A}/folder/123412341234", D, "box.folder:app.box.com/123412341234", "Customer Contracts"+P, "Customer Contracts", why="logged-in folder view"),
 pos("folder.view.query", f"{A}/folder/123412341234?tc=collab-folder-invite-treatment", D, "box.folder:app.box.com/123412341234", why="query noise"),
 pos("folder.view.slash_fragment", f"{E}/folder/234523452345/#section", D, "box.folder:acme.app.box.com/234523452345", why="trailing slash and fragment"),
 pos("folder.two_digit", f"{A}/folder/12", D, "box.folder:app.box.com/12", why="int(2,19) lower bound"),
 pos("folder.in_shared", f"{A}/s/abcdefghij0123456789/folder/345634563456", D, "box.folder:app.box.com/345634563456", "Team Docs"+P, "Team Docs", why="subfolder of a shared folder (title may be the share root's name)"),
 pos("folder.in_shared.sort", f"{A}/s/abcdefghij0123456789/folder/345634563456?sortColumn=date&sortDirection=ASC", D, "box.folder:app.box.com/345634563456", why="sort query"),
 pos("folder.in_vanity", f"{E}/v/media/folder/456745674567", D, "box.folder:acme.app.box.com/456745674567", "Media"+P, "Media", why="subfolder of a custom-URL folder, observed form"),
 pos("folder.enterprise_title_fallback", f"{E}/folder/567856785678", D, "box.folder:acme.app.box.com/567856785678", "Acme Corp"+P, "Acme Corp", why="KNOWN LIMIT: while untitled Box shows the enterprise name; recorded as the title"),
 # box.share
 pos("shared.s32", f"{A}/s/abcd1234efgh5678ijkl9012mnop3456", S, "box.share:app.box.com/s/abcd1234efgh5678ijkl9012mnop3456", "Deck.pdf"+P, "Deck.pdf", why="current 32-char shared name"),
 pos("shared.s20.slash", f"{A}/s/abcdefghij0123456789/", S, "box.share:app.box.com/s/abcdefghij0123456789", why="older 20-char name, trailing slash"),
 pos("shared.s10", f"{A}/s/abcde12345", S, "box.share:app.box.com/s/abcde12345", why="10-char legacy name (seen after /shared/ redirect)"),
 pos("shared.upper", f"{A}/s/ABCDEFGHIJ0123456789", S, "box.share:app.box.com/s/abcdefghij0123456789", why="names are case-insensitive: lowered"),
 pos("shared.query", f"{A}/s/abcdefghij0123456789?page=2&sortColumn=name#x", S, "box.share:app.box.com/s/abcdefghij0123456789", why="query and fragment ignored"),
 pos("shared.enterprise", f"{E}/s/zyxwvutsrq9876543210zyxwvutsrq98", S, "box.share:acme.app.box.com/s/zyxwvutsrq9876543210zyxwvutsrq98", "Team Docs"+P, "Team Docs", why="shared folder root on custom subdomain"),
 pos("shared.vanity", f"{E}/v/Press-Kit_2026", S, "box.share:acme.app.box.com/v/press-kit_2026", "Press Kit"+P, "Press Kit", why="custom URL, lowered"),
 pos("shared.vanity.slash", f"{E}/v/press-kit_2026/", S, "box.share:acme.app.box.com/v/press-kit_2026", why="vanity trailing slash"),
 pos("shared.vanity.short", f"{A}/v/UNL", S, "box.share:app.box.com/v/unl", why="short vanity name"),
 # negatives
 neg("neg.root", f"{A}/", "app root"),
 neg("neg.all_files", f"{A}/folder/0", "All Files root excluded by int(2,19)"),
 neg("neg.folder_search", f"{A}/folder/0/search?query=budget", "search"),
 neg("neg.folder_sub_search", f"{A}/folder/123412341234/search?query=x", "search inside folder (no rest)"),
 neg("neg.recents", f"{A}/recents", "recents list"),
 neg("neg.trash", f"{A}/trash", "trash"),
 neg("neg.favorites", f"{A}/files/favorites", "favorites list"),
 neg("neg.notes_create", f"{A}/notes/create_note", "create-note action, not an ID"),
 neg("neg.file_subroute", f"{A}/file/111122223333/versions", "sub-route under a file (no rest)"),
 neg("neg.legacy_files", f"{A}/files/0/f/2177659772/1/f_18817194544", "legacy hash-era file URL not expressible"),
 neg("neg.legacy_shared_tree", f"{A}/s/abcdefghij0123456789/1/1111111111/2222222222/1", "legacy shared tree (404 today)"),
 neg("neg.shared_static", f"{A}/shared/static/abcdefghij0123456789.pdf", "direct download"),
 neg("neg.shared_extra", f"{A}/s/abcdefghij0123456789/preview", "unknown sub-route under a shared link (no rest)"),
 neg("neg.vanity_dot", f"{A}/v/press.kit", "'.' is outside token: not matched (vanity alphabet assumption)"),
 neg("neg.embed", f"{A}/embed/s/abcdefghij0123456789", "embed iframe"),
 neg("neg.web_link", f"{A}/web_link/123412341234", "Box bookmark that opens another site"),
 neg("neg.hubs", f"{A}/hubs/123412341234", "Box Hubs not in this starter"),
 neg("neg.upper_route", f"{A}/S/abcdefghij0123456789", "uppercase route is 404 on Box; paths are case-sensitive"),
 neg("neg.master", f"{A}/master/settings/accountBilling", "admin console"),
 neg("neg.login", "https://account.box.com/login?redirect_url=%2Ffile%2F111122223333", "login host"),
 neg("neg.www", "https://www.box.com/s/abcdefghij0123456789", "marketing host, not an adapter origin"),
 neg("neg.file_noint", f"{A}/file/abc", "non-numeric file ID"),
 neg("neg.officeonline_noid", f"{A}/integrations/officeonline/openOfficeOnline?sharedAccessCode=", "no fileId"),
]
out = {"spec": "adapter-templates.md format 1; synthetic IDs only", "doc": "adapters.toml",
       "note": "Positive cases with a title give the expected recorded title; when no title entry matches, Brauser records the whole tab title.",
       "cases": cases}
pathlib.Path(__file__).with_name("vectors.json").write_text(json.dumps(out, indent=1, ensure_ascii=False) + "\n")
print(len([c for c in cases if c["expect"]["match"]]), "positive", len([c for c in cases if not c["expect"]["match"]]), "negative")

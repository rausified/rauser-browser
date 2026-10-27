A configuration and notes folder written by a build from before the fixed
home-folder layout, with its paths replaced by `@ROOT@` and `@ROOT_IDENTITY@`
and its page notes rewritten to example.com/example.org addresses.
Only `pages/` exists on disk, as in a real install that had saved page notes
but not yet captured any visits. The layout migration tests copy `home/` to a
temporary folder and fill in those two values.

# Docs

> The scripting API as a table, straight from scripting-api.md

This directory demonstrates a `yio` plugin. Before anything else, every `yio` run here draws a table of the scripting language's keywords and built-in functions. The plugin that draws it is `.yio/pre/0000_api.yio`, and it reads the rows out of `scripting-api.md`, so the table is never behind the document.

Set `API` to keep only the rows that mention some text, in the call, the group, or the description:

    API=file yio

In PowerShell that is `$env:API = 'file'; yio`. Matching is exact, so `File` and `file` differ.

Run `yio --cache` once and the table is replayed from `.yio/cache.yio` without running the plugin, until `scripting-api.md`, the plugin, or `API` changes.

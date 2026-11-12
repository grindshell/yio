# Docs

> Tasks for the documentation in this directory

## search [...TERMS]

> Show the scripting API, or only the calls that mention every word

This command does not draw the table. A plugin, `.yio/pre/0000_api.yio`, does, before the command runs. A plugin is told the command line, so it sees `search` and the words after it, and it reads the rows out of `scripting-api.md`, so the table is never behind the document.

A row is kept when the call, its group, or its description mentions every word. Matching is exact, so `File` and `file` differ.

With a cache (`yio --cache`), repeating a search replays the table without running the plugin, until `scripting-api.md` or the plugin changes.

```bash
# Nothing left to do: the plugin has already drawn the table.
```

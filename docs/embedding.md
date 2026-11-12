# Embedding

A `yio.md` can pull in the commands of another `yio.md`. The other file is loaded as part of the same run; no second `yio` process is started.

## Syntax

A paragraph that is nothing but a relative link to a file named `yio.md` is an embed:

```markdown
## docs

[docs](docs/yio.md)
```

`yio docs build` now runs the `build` command defined in `docs/yio.md`.

- The link text is ignored.
- The target is relative to the file that contains the link, not to the directory `yio` was started in. `..` is allowed.
- The file name must be exactly `yio.md`. A lone link to anything else (`[readme](README.md)`), an absolute path, or a URL is ordinary help text, as is a link with other text around it.

## What gets embedded

The embedded file's top-level (H2) commands become subcommands of the heading the link is under, with their own subcommands, arguments, flags, and scripts intact.

- **Under an H2 or deeper**, they nest below that command.
- **Under the H1, or before any heading**, they become top-level commands of the embedding file.
- **Help**: the embedded file's blockquote and paragraphs (the ones above its first command) describe the embedding heading, unless that heading has its own. Short and long help are filled in independently.
- **Mixing** is allowed. The embedding heading can have its own code blocks, flags, subheadings, and more than one embed. A command name that ends up defined twice under one heading is an error, whichever file it came from.
- The embedded file's H1 text is ignored, as it is everywhere.

One file may be embedded in several places. Each place gets its own copy of the commands.

## Where embedded scripts run

A script runs in the directory of the `yio.md` that defines it. `yio docs build` from the project root and `yio build` from inside `docs/` run the same script in the same directory, so a `yio.md` behaves the same whether it is used directly or embedded.

Commands written in the embedding file, including the heading that holds the link, still run in that file's directory.

## Errors

Errors name the file and line they are in:

```
yio: docs/yio.md: line 12: flag `JOBS` is missing its `flags` option
```

- A link whose file cannot be read is an error at the link, not a silently missing command.
- A file may not embed itself, directly or through other files. Files are compared by path after resolving `.` and `..`, without consulting the file system.
- Embeds may nest at most 32 files deep. That limit only exists to stop loops that paths cannot reveal, such as a symlink pointing at a parent directory.

## Plugins and the cache

An embedded directory's `.yio/pre/` and `.yio/post/` plugins run when its `yio.md` is embedded, in that directory. The cache of the directory `yio` was started in covers the embedded files and those plugins; the embedded directory's own `.yio/cache.yio` is only used when `yio` is run from there. See `scripting-language.md`.

## Not implemented

- Embedding part of a file (`docs/yio.md#section`).

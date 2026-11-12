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
- The link may name one section of the file. See "Embedding one command" below.

## What gets embedded

The embedded file's top-level (H2) commands become subcommands of the heading the link is under, with their own subcommands, arguments, flags, and scripts intact.

- **Under an H2 or deeper**, they nest below that command.
- **Under the H1, or before any heading**, they become top-level commands of the embedding file.
- **Help**: the embedded file's blockquote and paragraphs (the ones above its first command) describe the embedding heading, unless that heading has its own. Short and long help are filled in independently.
- **Mixing** is allowed. The embedding heading can have its own code blocks, flags, subheadings, and more than one embed. A command name that ends up defined twice under one heading is an error, whichever file it came from.
- The embedded file's H1 text is ignored, as it is everywhere.

One file may be embedded in several places. Each place gets its own copy of the commands.

## Embedding one command

A link that ends in `#` and a heading's anchor embeds only that command:

```markdown
## docs

[search](docs/yio.md#search-terms)
```

`yio docs search` now exists, and the other commands of `docs/yio.md` do not.

- **The anchor** is the one GitHub and most Markdown renderers give the heading, so the link also works when the file is read as a document. It is the heading's text in lower case, with spaces as hyphens and punctuation dropped. The arguments are part of the heading, so they are part of the anchor:

  | Heading | Anchor |
  | --- | --- |
  | `## build` | `#build` |
  | `## build (TARGET) [PROFILE]` | `#build-target-profile` |
  | `## search [...TERMS]` | `#search-terms` |
  | A second heading with the text `clean` | `#clean-1` |

- **What is embedded** is the command itself, under its own name, with its help, arguments, flags, scripts, and subcommands. It becomes a subcommand of the heading the link is under, or a top-level command when the link is under the H1 or before any heading.
- **Any depth**: the anchor can be that of an H3 or deeper, which lifts a subcommand out of its parent.
- **Help**: the command brings its own. The file's opening blockquote and paragraphs describe the whole file, so they are not passed on.
- **Several sections** of one file can be embedded, each with its own link.
- **The rest of the file** still has to load: an error anywhere in it is an error. But links outside the wanted section are not followed, so the files they point at are not read and their plugins do not run.
- A link with nothing after the `#` embeds the whole file.

No heading having that anchor is an error at the link. So is an anchor that belongs to the H1, which is not a command.
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
- That holds for sections too. A file that is being loaded cannot be embedded again, in whole or in part, so `a/yio.md` embedding `b/yio.md#x` whose section `x` embeds any part of `a/yio.md` is a cycle:

  ```
  yio: b/yio.md: line 3: `a/yio.md` is already being loaded; embeds cannot form a cycle
  ```

- A file may not embed a section of itself. `[build](#build)` and `[build](yio.md#build)` are both errors, wherever in the file they are.- Embeds may nest at most 32 files deep. That limit only exists to stop loops that paths cannot reveal, such as a symlink pointing at a parent directory.

## Plugins and the cache

An embedded directory's `.yio/pre/` and `.yio/post/` plugins run when its `yio.md` is embedded, in that directory. The cache of the directory `yio` was started in covers the embedded files and those plugins; the embedded directory's own `.yio/cache.yio` is only used when `yio` is run from there. See [plugins.md](plugins.md) and [cache.md](cache.md).

An embedded directory's plugins are told the part of the command line that follows the command its `yio.md` is embedded under. See "What plugins are told" in [plugins.md](plugins.md).


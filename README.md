# Y I Oughta (`yio`)

A task runner for `bash`, driven by a Markdown file. Read the name literally: "Why I oughta `build web prod`".

You write your tasks in a `yio.md` next to your project. Headings are commands, code blocks are the scripts they run, and everything else is the help text. The file reads as documentation and runs as a CLI.

`yio` is built for its author's own use first, and takes heavy inspiration from [`mask`](https://github.com/jacobdeichert/mask).

## Install

Prebuilt binaries for Linux, macOS, and Windows are attached to each [release](https://github.com/grindshell/yio/releases). Unpack the archive for your platform and put `yio` on your `PATH`.

Or build from source with a recent stable Rust:

```bash
cargo install --git https://github.com/grindshell/yio
```

`yio` runs scripts with the first `bash` on your `PATH`. On Windows that can be Git Bash or WSL.

## A first `yio.md`

````markdown
# Tasks

## build (TARGET) [PROFILE]

> Build a target

Longer description, shown with `--verbose`.

**FLAGS**
- JOBS
  - flags: `-j --jobs`
  - default: 4
  - description: Number of parallel jobs
- RELEASE
  - flags: `--release`
  - type: boolean

```bash
echo "building $TARGET ($PROFILE) with $JOBS jobs, release=$RELEASE"
```

### clean

> Remove build output

```bash
rm -rf out/
```
````

Then, in that directory:

```bash
yio
```

lists the commands,

```bash
yio build web prod -j 8 --release
```

runs one, and

```bash
yio build --help
```

describes one.

## The format

| In `yio.md` | Means |
| --- | --- |
| `## name` | A command. Deeper headings are subcommands of the nearest shallower one. |
| `> text` | Short help. |
| Paragraphs | Long help, shown with `--verbose`. |
| A fenced code block | The script. It always runs with `bash`; the `bash` tag is optional. A heading may have several, each run in its own process. |
| `(NAME)` in a heading | A required argument. |
| `[NAME]` in a heading | An optional argument. |
| `**FLAGS**` and a list | Named flags, each with `flags`, and optionally `type`, `default`, and `description`. |
| A paragraph that is only a link to another `yio.md` | Embeds that file: its commands nest under the heading. |

Arguments and flags reach the script as environment variables, under the name exactly as written.

Every command also accepts these flags:

| Flag | Variable | Does |
| --- | --- | --- |
| `-h`, `--help` | `YIO_HELP` | Describes the command instead of running it |
| `-V`, `--verbose` | `YIO_VERBOSE` | Includes the long help text |
| `-v`, `--version` | `YIO_VERSION` | Prints the version |
| `--cache` | `YIO_CACHE` | Writes `.yio/cache.yio` |
| `--remove-cache` | `YIO_REMOVE_CACHE` | Removes it |

The full rules are in [docs/cli-parsing.md](docs/cli-parsing.md), and embedding in [docs/embedding.md](docs/embedding.md).

## Plugins

Scripts in `.yio/pre/` run before the `yio.md` beside them is read, and scripts in `.yio/post/` after it has been loaded. They are named `NUMBER_NAME.bash` or `NUMBER_NAME.yio` and run in order of the number.

A `.bash` plugin is an ordinary bash script. A `.yio` plugin is written in a small Lisp-like language made for reading files and showing what it found. [docs/scripting-api.md](docs/scripting-api.md) is its reference. This one, at `docs/.yio/pre/0000_build_view.yio`, prints a table of the files in `docs/lore/` every time `yio` is used in `docs/`:

```lisp
(var lore-files (list-dir "lore"))

(if (is-empty lore-files)
  return)

(fn read-status (file)
  (var output ())
  (map (lines (read-file file))
    (fn (line)
      (if (starts-with line "status:")
        (set output (trim (replace line "status:" ""))))))
  output)

(col
  (col-def "File path" lore-files)
  (col-def "File size" (map lore-files file-size))
  (col-def "Status" (map lore-files read-status)))
```

```
File path  File size  Status
---------  ---------  ------
lore/a.md  19         draft
lore/b.md  15
```

This repository has a working one. Run `yio` in [docs/](docs/yio.md) and a plugin draws the scripting API as a table, read out of the reference itself. Set `API` to filter it:

```
API=dir yio
```

```
5 of 53 calls mention "dir"
Call               Group           Gives
-----------------  --------------  ---------------------------------------------------------------
(list-dir [path])  Reading files   The entries of a directory, sorted by name
(is-dir path)      Reading files   Whether the path is a directory
(make-dir path)    Changing files  Makes a directory, and any missing above it, and gives the path
(move from to)     Changing files  Moves a file or directory and gives the new path
(remove path)      Changing files  Removes a file or an empty directory and gives ()
```

## The cache

```bash
yio --cache
```

writes `.yio/cache.yio`. While that file exists, `yio` keeps it current and uses it:

- The command tree is read from it instead of from the Markdown.
- A `.yio` plugin is not run at all while the files it read are unchanged. Its stored result is shown instead.

Every built-in of the scripting language records what it looked at, which is what makes that safe: a changed file reruns exactly the plugins that read it, and a stale result is never shown. Bash plugins can read anything, so they always run.

With the table plugin above over 1000 files, a run takes 138 ms without the cache and 15 ms with it.

The cache describes one machine's files, so add `.yio/cache.yio` to your `.gitignore`.

How the cache decides what is out of date is in [docs/cache.md](docs/cache.md), and when plugins run in [docs/plugins.md](docs/plugins.md).

## Development

```bash
cargo test
```

```bash
cargo clippy --all-targets -- -D warnings
```

The crate has no dependencies, and keeping it that way is a project rule. [AGENTS.md](AGENTS.md) describes the layout and conventions.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your option.

# Y I Oughta (`yio`)

A task runner for `bash`, driven by a Markdown file (`yio.md`). Read the name literally: "Why I oughta `build web prod`". The binary is `yio` because it is an easy sequence to type on QWERTY.

The tool is built for its author's own use first. It is FOSS (dual MIT / Apache-2.0) and hosted at `github.com/grindshell/yio` so GitHub Actions can produce binary releases.

The file format is heavily inspired by [`mask`](https://github.com/jacobdeichert/mask). Where this document is silent, `mask`'s behavior is a reasonable reference, but ask before copying it.

## Status

A Rust crate (edition 2024) with no dependencies. The CLI scaffold exists: it loads `yio.md` and the files it embeds, resolves commands, arguments, and flags, lists commands, runs scripts with bash, and caches the command tree. `.yio/` plugins, the scripting language, and `.yiorc` are design only. Check `docs/` and the code before assuming a feature exists.

| File | Role |
| --- | --- |
| `src/command.rs` | The command tree and the built-in flags |
| `src/markdown.rs` | `yio.md` to command tree, including embeds |
| `src/cli.rs` | Command-line arguments to a resolved command and its values |
| `src/help.rs` | Command listings |
| `src/run.rs` | Runs a script with bash |
| `src/cache.rs` | `.yio/cache.yio`: encoding, input fingerprints, dirtiness check |
| `src/script/reader.rs` | `.yio` source bytes to forms |
| `src/script/eval.rs` | Evaluates forms: keywords, scopes, functions |
| `src/script/value.rs` | Script values |
| `src/script/builtins.rs` | Built-in functions (pure core only so far) |
| `docs/cli-parsing.md` | The parsing and running rules the above implement |
| `docs/embedding.md` | How one `yio.md` embeds another |
| `docs/scripting-language.md` | Plan for the scripting language, cache, and plugins. The reader, evaluator, and command tree cache are implemented; the CLI does not run scripts yet |

## Repo rules

- **Keep the dependency tree as lean as possible.** Prefer `std` and a small amount of hand-written code over a crate. Adding any dependency (including dev/build deps and enabling non-default features) needs a stated justification and the user's sign-off. This applies to the Markdown parser, argument parser, and serialization for the cache.
- **`docs/` holds design, implementation, and update docs.** Design decisions go there before or alongside the code that implements them. When behavior changes, update the relevant doc in the same change.
- Licensing is MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`); new files and any vendored code must be compatible.
- Do not guess on the items under [Open questions](#open-questions); ask.

## Commands

```bash
cargo build
cargo test
cargo run -- <args>
```

## The `yio.md` format

- **H2** headings are commands. **H3 and deeper** are subcommands, each nesting under the nearest shallower heading.
- **Blockquote** (`>`) under a heading: short help text.
- **Paragraphs** under a heading: verbose help text, shown with `--verbose` / `-V`.
- **Fenced code block**: the script to run. Always executed with `bash`; the `bash` info string is optional and only an editor hint. A future `.yiorc` may allow other default shells or runners. A heading may have several code blocks: they run in order, each in a separate, reinitialized environment.
- **Positional arguments**: `(MY_ARGUMENT_NAME)` in the heading. Required.
- **Optional arguments**: `[MY_ARGUMENT_NAME]` in the heading. The script decides whether the variable holds a value.
- **Named flags**: a paragraph containing `**FLAGS**`, immediately followed (newline optional) by a `-` delimited list. Each top-level item is one flag. Sub-items configure it with the keys `flags`, `type`, `default`, and `description`. Only `flags` is mandatory, the type defaults to string, and flags may have default values (unlike `mask`).
- **Embedding**: when a relative link to a file named `yio.md` is the only content of a heading's paragraph, that file is loaded in place as part of the same run and its commands nest under the heading. No second `yio` process is spawned. Embedded scripts run in their own file's directory. See `docs/embedding.md`.

Arguments and flags are injected into the script's environment under the name exactly as written. There is no automatic case conversion. Because Windows environment names are case-insensitive, names within one command must be unique ignoring case.

Example:

````markdown
## build (TARGET) [PROFILE]

> Build a target

Longer description shown with `--verbose`.

**FLAGS**
- JOBS
  - flags: `-j --jobs`
  - type: string
  - default: 4
  - description: Number of parallel jobs

```bash
echo "building $TARGET ($PROFILE) with $JOBS jobs"
```

### clean

> Remove build output

```bash
rm -rf out/
```

## docs

[docs](docs/yio.md)
````

### Built-in flags

Present on every command; a `yio.md` may override them.

| Variable | Flags | Type |
| --- | --- | --- |
| `YIO_HELP` | `-h --help` | boolean |
| `YIO_VERBOSE` | `-V --verbose` | boolean |
| `YIO_VERSION` | `-v --version` | string (semver) |
| `YIO_CACHE` | `--cache` | boolean |
| `YIO_REMOVE_CACHE` | `--remove-cache` | boolean |

- `YIO_VERSION` is the CLI's own Cargo semver string. `--version` prints it, and it is always injected into scripts.
- Booleans are the strings `"true"` and `"false"`. The CLI type-checks them, including usage inside scripts where that is relatively trivial to do.
- Passing `--cache` and `--remove-cache` together is an error.

## CLI behavior

- `yio` with no arguments, in a directory containing `yio.md`: parse the file and write the top-level commands and options to stdout. (This differs from `mask`.)
- `yio --help` / `-h`: show a formatted description of the given command or subcommand instead of running it. With no command, and with no `yio.md`, it describes the CLI itself.
- `yio --cache`: write the interpreted result to `.yio/cache.yio`, a cached AST of the commands, subcommands, and options, and exit.
- `yio --remove-cache`: remove that cache and exit.
- While the cache file exists it is used when its inputs are unchanged, and rewritten when they are not. A stale cache is never used.

### Cache format

`cache.yio` shares the scripting language's format where possible: the file is a single `(cache <raw bytes>)` expression whose bytes are the compressed AST. Scripts may be able to generate and pass around that compressed AST explicitly.

The byte encoding is a custom format optimized for interpretation speed, not for size or readability.

## The `.yio/` directory

Local to the current working directory, next to the `yio.md` it belongs to.

```
.yio/
  cache.yio   cached AST
  pre/        plugins run before the sibling yio.md is processed
  post/       plugins run after the sibling yio.md is processed
```

- Plugins run whenever the `yio.md` in the parent directory is accessed.
- Plugin file names are `NUMBER_NAME.EXT`. `NUMBER` is parsed as a `u64` (so `0000_build_view` is 0) and `NAME` is used for logging.
- The extension picks the runner: `.bash` for bash scripts, `.yio` for the yio scripting language (below).
- Plugins run at cache time by default. A plugin that should not must opt out itself by checking `YIO_CACHE`.

## Scripting language

A small Lisp-inspired language for plugins that generate dynamic output. The motivating case is a project's `docs/.yio/pre/0000_build_view.yio` that scans files and renders a table.

Two design constraints drive every decision about the language and its built-ins:

1. **Specialized for file manipulation.** It is not a general-purpose language.
2. **Cacheable.** A non-dirty run must be able to replay its result nearly instantly with no recomputation. Built-ins need to be designed so their inputs (files, directories) can be tracked for dirtiness.

Known syntax and built-ins, from the design sketch:

```lisp
-- comments start with two dashes

-- list-dir returns a list of files/subdirectories; no args means the current directory
(var lore-files (list-dir "lore"))

(if (is-empty lore-files)
  return)

-- (fn <name> (<optional args>) <body>); the last expression is implicitly returned
(fn read-status (file)
  -- () is the empty value
  (var output ())
  output)

-- col starts a column group as part of a table; col-def defines one column
(col
  (col-def
    "File path"
    -- an anonymous fn can be called immediately
    ((fn (files) files) lore-files))
  (col-def
    "Status"
    -- map applies a function to each value; it only loops over lists
    (map lore-files read-status)))
```

Also planned: text-manipulation built-ins for parsing file contents.

## Open questions

Not yet decided. Ask rather than assume.

- The scripting language decisions listed at the end of `docs/scripting-language.md`: reassignment, the script output model, how plugins interact with the cache, when `post/` runs, and where scripts may read and write.
- Scope and format of `.yiorc`.

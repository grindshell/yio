# CLI parsing

How `yio` turns a `yio.md` file and a command line into a script to run. This describes the scaffold in `src/`; the format itself is specified in `AGENTS.md`.

## Pipeline

1. `markdown.rs` reads `yio.md` from the current directory into a `Command` tree (`command.rs`). The root is the file; H2 headings are its subcommands, deeper headings nest under the nearest shallower one. Files it embeds are loaded into the same tree (see `embedding.md`).
2. `cli.rs` walks the arguments against that tree and produces an `Invocation`: the resolved command, the built-in flags that were passed, and a value for every argument and flag of that command.
3. `main.rs` acts on it. A command with code blocks is run by `run.rs`; a command without one is described by `help.rs`. `yio` with no arguments resolves to the root, which never has a script, so it lists the top-level commands.

Nothing about the commands is known at compile time, so there is no argument-parsing crate and no derive step. The parser is a single pass over the arguments.

## Resolution rules

- **Subcommands before arguments.** A word that matches a subcommand of the current command descends into it. The first word that does not becomes a positional argument, and from then on every word is positional. `yio build clean` runs `build clean`; `yio build web clean` runs `build` with two arguments.
- **`--` ends everything but positionals.** `yio build -- clean` passes `clean` as an argument, and `yio build -- -x` passes `-x`.
- **Flags are not inherited.** A flag belongs to the command it is defined on. Giving a parent's flag before a subcommand name (`yio build --release clean`) is an error rather than being dropped.
- **Built-ins work anywhere**, at any depth and in any position. A command's own flags are looked up first, so a flag that reuses `-v` or the name `YIO_VERBOSE` replaces the built-in for that command.
- **Arity is checked.** A missing `(REQUIRED)` argument and an argument beyond the declared ones are both errors. Required arguments cannot be declared after optional ones.

## Help

`--help` / `-h` describes the resolved command instead of running it: its short help, usage, subcommands, and flags. Paragraph text is added with `--verbose`, for the command itself and for each listed subcommand. It takes precedence over `--version`. Neither runs the command, so both work without its required arguments: `yio build --help`, `yio build --version`.

Usage is shown as one form per way of invoking the command, because a subcommand is only recognized before the first argument:

```
Usage: yio build (TARGET) [PROFILE] [flags]
       yio build <command> [flags]
```

The first form is left out when the command has subcommands and nothing to run; the second when it has no subcommands.

The flag list shows what the parser will actually do. A command's flag shadows a built-in one spelling at a time, so a command that defines `-h --hot` lists the built-in as `--help` only, and a built-in with every spelling shadowed is not listed.

`yio --help` describes the root. Without a `yio.md` that is the CLI on its own: a one-line description and the built-in flags. Every other invocation without a `yio.md` is an error, apart from `--version`.

A command with no code block has nothing to run, so naming it prints the same description.

## Running

The bash that runs is the first one on `PATH`. On Windows `yio` searches `PATH` for `bash.exe` itself, because the standard library looks in the system directories first and would pick the WSL launcher in System32 over a bash that comes earlier on `PATH`.

That launcher (`C:\Windows\System32\bash.exe`) does not pass its arguments to bash unchanged. It sends the rest of its command line through a shell inside WSL, so a script given as an ordinary `-c` argument is expanded twice: `echo '$HOME'` prints a path and `awk '{print $2}'` loses its `$2`. When the bash found is in the system directory, `yio` writes the script onto the command line itself as one single-quoted shell word, which that first shell leaves alone. Every other bash gets the script as a normal argument.

Scripts run in the directory of the `yio.md` that defines them, which matters for embedded files (see `embedding.md`).

Each code block of a command runs as its own `bash -c` process, in the order written. They share the injected variables and nothing else: shell variables, options, and the working directory set by one block do not reach the next. The first block that exits non-zero stops the run, and its exit code becomes `yio`'s.

## Flag spellings

| Form | Example |
| --- | --- |
| Long, separate value | `--jobs 8` |
| Long, inline value | `--jobs=8` |
| Short, separate value | `-j 8` |
| Short, attached value | `-j8`, `-j=8` |
| Short cluster | `-rj 8` (every flag but the last must be boolean) |

A string flag always consumes the next argument as its value, even one that starts with `-`. A boolean flag never consumes one: bare means `true`, and `--release=true` / `--release=false` are the only explicit forms. Anything else is a type error. When a flag repeats, the last occurrence wins.

## Values injected into the script

Every argument and flag of the resolved command is set, so a script never sees a value leaked from the calling shell:

| Kind | Not given |
| --- | --- |
| Optional argument | empty string |
| String flag | its `default`, else empty string |
| Boolean flag | its `default`, else `false` |

`YIO_HELP`, `YIO_VERBOSE`, `YIO_CACHE`, and `YIO_REMOVE_CACHE` are always set to `true` or `false`, and `YIO_VERSION` to the CLI's version.

On Windows the injected names are also appended to `WSLENV`, because the `bash` on `PATH` is often the WSL launcher and it forwards nothing else. The name `WSLENV` is therefore reserved.

### Variable names

Names are injected exactly as written, but Windows treats environment variable names as case-insensitive, so `Name` and `NAME` would silently become one variable there. To keep a `yio.md` behaving the same on every platform, the loader rejects, on every platform:

- two arguments or flags of one command whose names differ only by case;
- a name that differs from a built-in only by case, such as `yio_version` (the exact name `YIO_VERSION` is an override and is allowed);
- `WSLENV`, in any case.

## What the loader accepts

- One optional H1, before any command. Its text is ignored; a blockquote and paragraphs under it describe the file as a whole.
- A heading is one command name followed by `(ARG)` and `[ARG]` tokens. Argument and flag names must be valid environment variable names.
- Any number of code blocks per command, each with no language or `bash`. Any other language is an error, so that a `json` example is never run as a script.
- A fence may be indented up to three spaces, and that indentation is removed from each line of the script, as Markdown does. Anything indented four or more spaces (or a tab) is literal text: it is never a script, heading, or marker, and it lands in the verbose help.
- An HTML comment that starts a line (`<!--`) hides everything through the line containing `-->`, so a commented-out code block or heading is ignored. A comment that is never closed is an error.
- `**FLAGS**` on its own line, then a list. Options are `flags` (required), `type` (`string` or `boolean`), `default`, and `description`. Values may be wrapped in backticks.

Errors name the file and line: `yio.md: line 12: flag `JOBS` is missing its `flags` option`.

## Not implemented

- Caching of script results. Plugins, the scripting language, and the command tree cache exist; see `scripting-language.md`.
- Type-checking boolean variables inside scripts.
- `.yiorc`.

# CLI parsing

How `yio` turns a `yio.md` file and a command line into a script to run. The format itself is summarized in `AGENTS.md`.

## Pipeline

1. `markdown.rs` reads `yio.md` from the current directory into a `Command` tree (`command.rs`). The root is the file; H2 headings are its subcommands, deeper headings nest under the nearest shallower one. Files it embeds are loaded into the same tree (see [embedding.md](embedding.md)).
2. `cli.rs` walks the arguments against that tree and produces an `Invocation`: the resolved command, the built-in flags that were passed, and a value for every argument and flag of that command.
3. `main.rs` acts on it. A command with code blocks is run by `run.rs`; a command without one is described by `help.rs`. `yio` with no arguments resolves to the root, which never has a script, so it lists the top-level commands.

Two things wrap around step 1. Plugins run before each `yio.md` is read and again once the tree is ready (see [plugins.md](plugins.md)). And when `.yio/cache.yio` exists and is current, the tree is decoded from it and the Markdown is not parsed (see [cache.md](cache.md)).

Nothing about the commands is known at compile time, so there is no argument-parsing crate and no derive step. The parser is a single pass over the arguments.

## Resolution rules

- **Subcommands before arguments.** A word that matches a subcommand of the current command descends into it. The first word that does not becomes a positional argument, and from then on every word is positional. `yio build clean` runs `build clean`; `yio build web clean` runs `build` with two arguments.
- **`--` ends everything but positionals.** `yio build -- clean` passes `clean` as an argument, and `yio build -- -x` passes `-x`.
- **Flags are not inherited.** A flag belongs to the command it is defined on. Giving a parent's flag before a subcommand name (`yio build --release clean`) is an error rather than being dropped.
- **Built-ins work anywhere**, at any depth and in any position. A command's own flags are looked up first, so a flag that reuses `-v` or the name `YIO_VERBOSE` replaces the built-in for that command.
- **Arity is checked.** A missing `(REQUIRED)` argument and an argument beyond the declared ones are both errors. Required arguments cannot be declared after optional ones.
- **`[...NAME]` takes the rest.** Once the required arguments are filled, every further argument goes to it. See below.

## The rest of the arguments

A heading may end with `[...NAME]`, which takes every argument that the ones before it leave over, from none to many:

```markdown
## search [...TERMS]

## tag (FILE) [...TAGS]
```

`yio search built-in functions` gives `TERMS` two values. `yio tag notes.md draft lore` gives `FILE` the first argument and `TAGS` the other two. `yio search` alone is fine and gives an empty list.

It is allowed only where it cannot be confused with anything else:

- It must be the last argument in the heading, and there can be one.
- The command may not have optional arguments. With `[A] [...B]` a missing `A` and an empty `B` could not be told apart from the script's side.
- The command may not have subcommands, in its own file or through an embed. Every word after the command is one of its arguments, so a subcommand could never be reached.

Flags are still recognized among the values, so `yio search -e built-in` sets the flag `-e`. A value that starts with `-` goes after `--`: `yio search -- -e`.

### In the script

An environment variable holds one string, so a list cannot be one. The script gets a bash array instead, under the name without the dots:

```bash
echo "${#TERMS[@]} terms"
for term in "${TERMS[@]}"; do
  grep -rn -- "$term" .
done
```

- `"${TERMS[@]}"` is each value as its own word, and `"${TERMS[*]}"` is all of them joined with spaces.
- Values arrive exactly as typed. Nothing in them is expanded or split, whatever they contain.
- `yio` puts the array's assignment at the start of the code block's first line, so the line numbers in bash's error messages still match the block. Every code block of the command gets it.
- The assignment replaces a variable of the same name inherited from the calling shell.
- Being an array, it is not passed on to programs the script starts. Hand it over as arguments: `my-tool "${TERMS[@]}"`.
- In bash older than 4.4, which includes the one macOS ships, expanding an empty array under `set -u` is an error. `${TERMS[@]+"${TERMS[@]}"}` avoids it.

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

Scripts run in the directory of the `yio.md` that defines them, which matters for embedded files (see [embedding.md](embedding.md)).

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
| `[...NAME]` | an empty bash array |

`YIO_HELP`, `YIO_VERBOSE`, `YIO_CACHE`, and `YIO_REMOVE_CACHE` are always set to `true` or `false`, and `YIO_VERSION` to the CLI's version.

On Windows the injected names are also appended to `WSLENV`, because the `bash` on `PATH` is often the WSL launcher and it forwards nothing else. The name `WSLENV` is therefore reserved.

### Variable names

Names are injected exactly as written, but Windows treats environment variable names as case-insensitive, so `Name` and `NAME` would silently become one variable there. To keep a `yio.md` behaving the same on every platform, the loader rejects, on every platform:

- two arguments or flags of one command whose names differ only by case;
- a name that differs from a built-in only by case, such as `yio_version` (the exact name `YIO_VERSION` is an override and is allowed);
- `WSLENV`, in any case.

## Boolean checks

A boolean flag reaches a script as the string `true` or `false`. It is never empty and never a number, which makes some natural-looking tests give the same answer whichever it is:

```bash
if [ -n "$RELEASE" ]; then   # always true: "false" is not empty
```

When there is a cache, `yio` looks for such uses and warns about them on stderr. They are warnings only: the command still runs, and its exit code is unchanged.

```
yio: warning: yio.md: line 11: `RELEASE` is always `true` or `false`, never empty, so this test gives the same answer either way; compare it with `true` instead: [ "$RELEASE" = true ]
```

- `yio --cache` reports every script in the tree, embedded files included.
- Running a command while `.yio/cache.yio` exists reports that command's scripts, each time.
- Without a cache nothing is checked.

The variables checked are the command's own boolean flags and `YIO_HELP`, `YIO_VERBOSE`, `YIO_CACHE`, and `YIO_REMOVE_CACHE`, except one whose name the command has given to an argument or to a flag of another type.

| Flagged | Why |
| --- | --- |
| `[ -n "$X" ]`, `[ -z "$X" ]`, `[ "$X" ]`, `[ "$X" != "" ]` | It is never empty |
| `[ "$X" = 1 ]`, `[ "$X" == yes ]` | It only ever equals `true` or `false` |
| `[ "$X" -eq 1 ]` and the other number comparisons | It is not a number |
| `(( X ))`, `$(( X + 1 ))` | Arithmetic reads `true` and `false` as unset variables, so both are 0 |
| `${X:+text}`, `${X+text}` | It is always set, so the text is used even for `false` |

The same holds in `[[ ]]` and `test`, with `!`, and for one condition among several joined by `&&`, `||`, `-a`, or `-o`.

| Not flagged | Why |
| --- | --- |
| `[ "$X" = true ]`, `[[ $X != false ]]` | This is the comparison that works |
| `if $X; then`, `"$X" && ...` | `true` and `false` are commands, so running it works |
| `case "$X" in` | |
| `--release="$X"`, `echo "$X"`, `other=$X` | Handing it on is not a test |
| `[ "$X" = "$OTHER" ]` | What it is compared with is only known when the script runs |
| `${X:-false}` | The default is never used, but it does no harm |

This is a heuristic over the text of a script, one line at a time. It is not a bash parser, and it reports only what it recognizes and is sure is wrong. So no warning does not mean a script is right. In particular it does not see:

- a test that continues onto the next line;
- a boolean that was first copied into another variable;
- how a function or a program the script calls uses the value.

It also takes the body of a here-document for code, so text there that looks like a test is reported.

Implemented in `src/check.rs`.

## What the loader accepts

- One optional H1, before any command. Its text is ignored; a blockquote and paragraphs under it describe the file as a whole.
- A heading is one command name followed by `(ARG)` and `[ARG]` tokens, optionally ending with `[...ARGS]` when there is no `[ARG]`. Argument and flag names must be valid environment variable names.
- Any number of code blocks per command, each with no language or `bash`. Any other language is an error, so that a `json` example is never run as a script.
- A fence may be indented up to three spaces, and that indentation is removed from each line of the script, as Markdown does. Anything indented four or more spaces (or a tab) is literal text: it is never a script, heading, or marker, and it lands in the verbose help.
- An HTML comment that starts a line (`<!--`) hides everything through the line containing `-->`, so a commented-out code block or heading is ignored. A comment that is never closed is an error.
- `**FLAGS**` on its own line, then a list. Options are `flags` (required), `type` (`string` or `boolean`), `default`, and `description`. Values may be wrapped in backticks.

Errors name the file and line: `yio.md: line 12: flag `JOBS` is missing its `flags` option`.

## Not implemented

- Checking booleans without a cache, and a way to silence a warning. See "Boolean checks".
- `.yiorc`, which is not designed yet.

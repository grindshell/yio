---
name: yio
description: Create, edit, debug, and run `yio.md` task files for `yio` ("Y I Oughta"), a task runner where Markdown headings are commands and fenced code blocks are the bash they run. Use this whenever a project has a `yio.md` or a `.yio/` directory, the user mentions yio, asks to add, change, or fix a task or command in such a project, wants a project's build, test, or deploy commands collected into one runnable Markdown file, or wants a `.yio` plugin or a per-directory status view. Use it even for a small request like "add a task for X" when the repo has a `yio.md`. The format looks like mask, just, or make but its rules differ, so read this before writing one.
---

# yio

`yio` reads the `yio.md` in the current directory and turns it into a CLI. Headings are commands, fenced code blocks are the bash scripts they run, and everything else is help text. The file reads as documentation and runs as a tool.

The `yio` binary is expected on `PATH`. If `yio --version` fails, say so and point to <https://github.com/grindshell/yio/releases>. Do not write a `yio.md` you could not check.

## How to work

`yio` is its own validator, so use it at each step instead of reasoning about what the loader would accept.

1. **Look.** Run `yio` in the directory to list the commands, and read the `yio.md`. Check for a `.yio/` directory: plugins there run on every use of the file (see "Plugins" below).
2. **Edit** the `yio.md`.
3. **Check.** Run `yio` again. A file that does not load fails with `yio: yio.md: line N: ...`, which says what to fix. Then `yio COMMAND --help` to see a command as a user will, and run the command when that is harmless.
4. **Check the booleans.** `yio --cache` reads every script and warns when one tests a boolean flag in a way that cannot work. It writes `.yio/cache.yio`, so if that file was not there before and the project does not want a cache (see [references/plugins.md](references/plugins.md) for when it does), finish with `yio --remove-cache`. That also removes a `.yio/` directory it leaves empty.

`--help` never runs a command's script, so it is safe on any command. It does still run the directory's plugins; only `--version` and `--remove-cache` run nothing.

There is no dry run. A script that is not safe to run is checked by reading it, by `--help`, and by the paths that stop early (a missing argument, a refused confirmation). `yio`'s own errors exit with 2; otherwise the exit code is the script's.

**On Windows, find out which bash will run before writing scripts.** `yio` uses the first `bash` on `PATH` (`Get-Command bash` or `where bash` shows it). If that is `C:\Windows\System32\bash.exe`, scripts run inside WSL: a Linux system, working directory under `/mnt/c/...`, where programs installed on the Windows side (`npm`, `cargo`) are usually not found. Git Bash sees the Windows programs. Tell the user which one their scripts will get when it matters.

## The file

````markdown
# Project tasks

> One line about the file, shown above the command list

## build (TARGET) [PROFILE]

> Build a target

Paragraphs are the long help, shown with `--verbose`.

**FLAGS**
- JOBS
  - flags: `-j --jobs`
  - default: 4
  - description: Number of parallel jobs
- RELEASE
  - flags: `-r --release`
  - type: boolean
  - description: Build with optimizations

```bash
set -e
echo "building $TARGET (${PROFILE:-dev}) with $JOBS jobs"
if [ "$RELEASE" = true ]; then
  echo "optimized"
fi
```

### clean

> Remove build output

```bash
rm -rf out/
```

## test [...NAMES]

> Run the tests, or only the ones named

```bash
run-tests "${NAMES[@]}"
```

## docs

[docs](docs/yio.md)
````

That gives `yio build web prod -j 8 --release`, `yio build clean`, `yio test parser lexer`, and everything in `docs/yio.md` under `yio docs`.

| Written as | Means |
| --- | --- |
| `# Title` | Optional, once, before any command. The text is ignored. A blockquote under it is shown above the command list, and paragraphs with `--verbose`. |
| `## name` | A command. `###` and deeper are subcommands of the nearest shallower heading. |
| `> text` | Short help: the one line shown beside the command in listings. |
| Paragraphs | Long help, shown with `--verbose` / `-V`. |
| A fenced code block | A script. The tag must be `bash` or nothing. |
| `(NAME)` in the heading | A required argument. |
| `[NAME]` in the heading | An optional argument. Must come after the required ones. |
| `[...NAME]` last in the heading | Takes every argument left over, none to many. |
| `**FLAGS**` then a list | Named flags. One top-level item per flag, sub-items configure it. |
| A paragraph that is only a link to another `yio.md` | Embeds that file's commands under this heading. |

Details that the loader enforces:

- **Headings** are a command name followed only by argument tokens. Anything else in the heading is an error. Only `#`-style headings count.
- **Flags.** The item text is the variable name. `flags` is required and takes one short form, one long form, or both (`-j --jobs`). `type` is `string` (the default) or `boolean`. `default` and `description` are optional. A boolean's default must be `true` or `false`. Flags belong to the command they are written under and are not inherited by subcommands.
- **Flags are always optional.** There is no `required` option, so a flag that must be given is checked by the script (see the confirmation example under "Writing a good one"). A `default` is shown in the help as `[default: 4]` automatically, so the description need not repeat it.
- **Names** of arguments and flags must be valid environment variable names, and unique within a command even ignoring case. `WSLENV` is reserved. Upper snake case is the convention. They become real environment variables, so do not use a name that already means something to bash or to the programs the script runs: `PATH`, `HOME`, `IFS`, `PWD`, `USER`, `SHELL`, `LANG`.
- **`[...NAME]`** needs a command with no optional arguments and no subcommands, because nothing could tell them apart on the command line.
- **A command with no code block** has nothing to run, so naming it prints its help. That is the normal shape for a heading that only groups subcommands.
- **Several code blocks** under one heading run in order, each as a separate `bash -c`. The first that fails stops the rest.
- **To hide something** from both the commands and the help, wrap it in an HTML comment that starts a line (`<!--` ... `-->`).

## What a script gets

Each code block runs with `bash -c`, in the directory of the `yio.md` that defines it, wherever `yio` was started from.

| Kind | In the script | When not given |
| --- | --- | --- |
| `(NAME)`, `[NAME]` | `$NAME` | Required: an error before the script runs. Optional: empty string. |
| String flag | `$NAME` | Its `default`, else empty string |
| Boolean flag | `$NAME`, the string `true` or `false` | Its `default`, else `false` |
| `[...NAME]` | The bash array `NAME` | An empty array |

Arguments and flags are environment variables, so programs the script starts see them too: with a flag named `PORT`, `npm run dev` already has `PORT` in its environment. Every argument and flag of the command is always set, so a value never leaks in from the calling shell. `YIO_VERSION`, `YIO_HELP`, `YIO_VERBOSE`, `YIO_CACHE`, and `YIO_REMOVE_CACHE` are set as well.

Things that follow from this, and that are easy to get wrong:

- **Booleans are never empty.** Test one with `[ "$RELEASE" = true ]`, or run it: `if $RELEASE; then`. `[ -n "$RELEASE" ]` and `[ "$RELEASE" ]` are true for `false` too, and `(( RELEASE ))` is always 0.
- **There is no `set -e` by default.** A block carries on after a failing command and exits with the status of its last one. Start a multi-step block with `set -e`, or give each step a code block of its own, which also gives each a fresh shell.
- **Blocks share nothing** but the injected variables. A `cd`, a shell variable, or a function from one block is gone in the next.
- **The rest array** is `"${NAMES[@]}"` for each value as its own word, `"${NAMES[*]}"` for all joined by spaces, `${#NAMES[@]}` for the count. Values arrive exactly as typed. It is not exported, so pass it on as arguments. Under `set -u` with bash older than 4.4 (macOS ships 3.2), write `${NAMES[@]+"${NAMES[@]}"}`.
- **yio already checks** that required arguments are present, that there are no extra ones, and that boolean values are `true` or `false`. Scripts do not need to repeat that.

## How the command line is read

- A word that matches a subcommand descends into it. The first word that does not is an argument, and so is every word after it. `yio build clean` runs the subcommand; `yio build web clean` runs `build` with two arguments.
- `--` makes everything after it an argument: `yio run -- --version`.
- Flags can be written `--jobs 8`, `--jobs=8`, `-j 8`, `-j8`, and clustered as `-rj 8`. A boolean flag never takes the next word: bare means `true`, and `--release=false` or `--release=true` is the only way to give it a value. A string flag always takes the next word, even one that starts with `-`.
- A parent's flag before a subcommand name (`yio build --release clean`) is an error.
- The built-in flags work on every command: `-h --help`, `-V --verbose`, `-v --version`, `--cache`, `--remove-cache`. A command that defines a flag with one of those spellings replaces the built-in for itself, so avoid `-h`, `-v`, and `-V` for your own flags.

## Writing a good one

The file is read by people as often as it is run, so write it as the project's task documentation.

- Give every command a `>` line. It is all a listing shows: the name, the arguments, and that line. Flags appear only in the command's own `--help`, so a flag someone has to know about belongs in the `>` line.
- Use the paragraphs for what someone needs before running the command: what it changes, what it needs installed, an example invocation. Leave out notes to yourself, since paragraphs are shown to users. Help is printed as plain text, as written: backticks stay, and indentation is dropped.
- Prefer a flag with a `default` over an optional argument for a setting. Arguments suit the thing being acted on.
- Group related commands as subcommands under a heading with no code block (`## db`, `### migrate`, `### seed`).
- Quote expansions (`"$TARGET"`), as in any bash.
- To show example output in the help, indent it four spaces. A fenced block is always a script, and any tag other than `bash` is an error precisely so that an example is never run by accident.
- Keep long logic in a script file and call it from the code block. The `yio.md` stays readable and the script can be linted.

A command that must be confirmed checks its own flag, since flags cannot be required:

````markdown
## deploy (TARGET)

> Deploy to a target. Does nothing without --yes

**FLAGS**
- YES
  - flags: `-y --yes`
  - type: boolean
  - description: Confirm the deploy

```bash
set -e
if [ "$YES" != true ]; then
  echo "Refusing to deploy to $TARGET without --yes." >&2
  exit 1
fi
./scripts/deploy.sh "$TARGET"
```
````

## What goes wrong

| Symptom | Cause |
| --- | --- |
| "code block language ... is not supported" | Only `bash` or no tag. Indent non-script examples four spaces. |
| "unexpected ... after command" | The heading has words besides the name and `(A)` `[B]` `[...C]` tokens. |
| "required argument ... cannot follow an optional one" | Order is `(REQUIRED)` then `[OPTIONAL]`. |
| "`[...X]` cannot be used with the optional argument" | Make the optional one a flag. |
| "takes `[...X]`, so it cannot have the subcommand" | A rest argument and subcommands exclude each other, embedded ones included. |
| "flag `-r` belongs to `yio build`, not to its subcommand" | Flags are not inherited. Define the flag on the subcommand as well. |
| "differs from ... only by case" | Names must be unique ignoring case, because Windows environment names are. |
| A paragraph of help vanished and commands appeared | A paragraph that is only a link to a `yio.md` is an embed. Add words around the link to keep it as text. |
| A later step ran after an earlier one failed | No `set -e`. See above. |
| The command works alone but not through the parent directory | Scripts run in their own file's directory. Paths relative to where `yio` was typed do not hold. |
| On Windows, programs are not found | `yio` uses the first `bash` on `PATH`. The WSL launcher sees Linux's programs, Git Bash sees Windows's. |

## Splitting a project across directories

A paragraph that is nothing but a relative link to a file named `yio.md` embeds it, in the same process:

```markdown
## docs

[docs](docs/yio.md)
```

- The file's top-level commands become subcommands of the heading. Under the H1 or before any heading, they become top-level commands.
- If the heading has no help of its own, it takes the embedded file's opening blockquote and paragraphs.
- Embedded scripts run in their own directory, so `yio docs build` from the root and `yio build` inside `docs/` do the same thing.
- `[search](docs/yio.md#search-terms)` embeds one command. The anchor is the one GitHub gives the heading: lower case, spaces to hyphens, punctuation dropped, so `## search [...TERMS]` is `#search-terms`.
- A file cannot embed itself, in whole or in part, directly or through others. A command name defined twice under one heading is an error.

## Plugins and dynamic output

A `.yio/` directory beside a `yio.md` can hold scripts that run whenever that file is used: `.yio/pre/` before it is read, `.yio/post/` after. They are how a directory shows live information above its command list (a table of files and their status, a progress panel), generates part of its own `yio.md`, or answers a command with something computed. They are written in bash or in yio's own small Lisp-like language, whose results can be cached and replayed.

Read these when the task involves `.yio/`, the cache, or anything "dynamic":

- [references/plugins.md](references/plugins.md): naming, order, what a plugin is told, bash or `.yio`, the cache.
- [references/scripting.md](references/scripting.md): the `.yio` language, every built-in, tables, boxes, trees, bars, and styles.

Know that plugins exist even when not writing one: running `yio` in a directory runs its plugins, and they may write files.

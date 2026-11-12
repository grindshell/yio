# Plugins

Scripts in a directory's `.yio/pre/` and `.yio/post/` run whenever the `yio.md` beside that `.yio/` is used. Implemented in `src/plugin.rs`.

```
.yio/
  pre/        run before the yio.md is read
  post/       run after the command tree is ready
  cache.yio   see cache.md
```

## Which files are plugins

Every file in `.yio/pre/` and `.yio/post/` must be named `NUMBER_NAME.bash` or `NUMBER_NAME.yio`. `NUMBER` is a `u64`, and `NAME` is what `--verbose` logs. Anything else is an error, so a misnamed plugin is reported instead of silently never running. Names that start with a dot, such as `.gitkeep`, are left alone.

They run in order of `NUMBER`, then by file name. `0002_a` and `2_b` have the same number.

## When they run

| Stage | Runs |
| --- | --- |
| `pre` | Before the `yio.md` beside it is read. With a cache, before the cached tree is checked, because a `pre` plugin may rewrite the files that tree was built from. |
| `post` | Once the whole command tree is ready and before anything is listed or run. Directories go in the order their `yio.md` was reached, the starting directory first. |

- A plugin runs at most once per `yio` run.
- An embedded directory's plugins run when its `yio.md` is embedded: its `pre` plugins just before that file is read.
- Plugins run for listing, `--help`, running a command, and `--cache`. They do not run when `--version` or `--remove-cache` is on the command line, since neither uses the `yio.md`.
- A `pre` plugin may create or rewrite the `yio.md` it sits beside. It runs even when that file does not exist yet.
- A failing plugin stops the run. The error names the plugin's file.
- With `--verbose`, each plugin is announced on stderr by name before it runs, and one whose result came from the cache is announced as `from the cache`.

`post` runs after loading, not after the chosen command. That keeps plugins about the `yio.md`, and the command's exit code stays `yio`'s.

## The two kinds

| | `.bash` | `.yio` |
| --- | --- | --- |
| Run by | bash, as a script file | The evaluator |
| Working directory | The directory of the `yio.md` | Relative paths resolve against the same directory |
| Output | Whatever it prints | Its value, drawn by `yio` |
| Is told | The built-in flags in its environment and the command line as `"$@"` | The same through `env` and `args` |
| Fails when | It exits non-zero | It has an error, or its value contains a function |
| With a cache | Runs every time | Not run while its inputs are unchanged: its stored value is drawn instead |

Bash plugins are opaque, so they can never be replayed. That is the reason to write a plugin in the scripting language. Its reference is [scripting-api.md](scripting-api.md).

## What plugins are told

Two things: the built-in flags, and the rest of the command line.

| | `.bash` | `.yio` |
| --- | --- | --- |
| The built-in flags | `$YIO_HELP`, `$YIO_VERBOSE`, `$YIO_CACHE`, `$YIO_REMOVE_CACHE`, `$YIO_VERSION` | `(env "YIO_HELP")` and so on |
| The rest of the command line | `"$@"`: `$1`, `$2`, and so on | `(args)`, a list of strings |

### The built-in flags

Plugins run before the commands are known, so the flags come from the command line as written: a built-in flag counts if its spelling appears anywhere before `--`. This differs from the real parse in one case. If a command defines its own flag with a built-in's spelling, such as its own `--cache`, plugins are still told the built-in was given.

Plugins run during `yio --cache` too. One that should not checks `YIO_CACHE` itself and stops.

### The rest of the command line

Every other argument, in the order typed, each exactly as typed: the command names, their arguments, and their own flags. The built-in flags are left out, since they are told separately, and so the first word is the command whether `-V` came before it or not. A `--` is kept, and so is everything after it.

| Command line | A plugin is told |
| --- | --- |
| `yio` | nothing |
| `yio search file path` | `search` `file` `path` |
| `yio -V search --help file` | `search` `file` |
| `yio build web -j 8` | `build` `web` `-j` `8` |
| `yio search -- -V` | `search` `--` `-V` |

A plugin is not told how those words were understood: which are command names, which are values, or whether the command exists. `pre` plugins run before any of that is known. It is told the same words at both stages.

**In an embedded directory**, a plugin is told only what follows the command its `yio.md` is embedded under. With `docs/yio.md` embedded under `## docs`:

| Command line | The plugins of `docs/` are told |
| --- | --- |
| `yio docs search file` | `search` `file` |
| `yio docs` | nothing |
| `yio build web` | nothing |

So a plugin sees the same words whether `yio` is run in its own directory or its `yio.md` is reached through another: `yio search file` in `docs/` and `yio docs search file` from the directory above are the same to it. A command line that leads somewhere else gives it nothing.

- A `yio.md` embedded in several places is told by the first place it is reached.
- A `yio.md` embedded under the H1, or before any heading, is told everything, like the file that embeds it.
- A `.yio` plugin that calls `(args)` depends on them: with a cache, its stored result is reused only for the same words. One that never calls it is replayed whatever the command line is.

## An example

This directory has one. [`.yio/pre/0000_api.yio`](.yio/pre/0000_api.yio) answers the `search` command of [yio.md](yio.md). It reads the tables out of [scripting-api.md](scripting-api.md) and draws the rows that mention every word it was told:

```
$ yio search dir path
5 of 54 calls mention: dir path
Call               Group           Gives
-----------------  --------------  ---------------------------------------------------------------
(list-dir [path])  Reading files   The entries of a directory, sorted by name
(is-dir path)      Reading files   Whether the path is a directory
(make-dir path)    Changing files  Makes a directory, and any missing above it, and gives the path
(move from to)     Changing files  Moves a file or directory and gives the new path
(remove path)      Changing files  Removes a file or an empty directory and gives ()
```

It starts by looking at what it was told, and does nothing unless the first word is `search`:

```lisp
(var words (args))

(if (or (not (= (first words) "search")) (= (env "YIO_HELP") "true"))
  return)
```

Because `args` and `read-file` record what they looked at, a cached table is reused only for the same search and the same `scripting-api.md`.

The `search` command itself has a code block with nothing in it but a comment. A command needs one to be runnable, and the plugin has already done the work by the time it runs.
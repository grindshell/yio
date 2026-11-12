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
| Sees | The `YIO_*` variables in its environment | The same through `(env "YIO_CACHE")` and the like |
| Fails when | It exits non-zero | It has an error, or its value contains a function |
| With a cache | Runs every time | Not run while its inputs are unchanged: its stored value is drawn instead |

Bash plugins are opaque, so they can never be replayed. That is the reason to write a plugin in the scripting language. Its reference is [scripting-api.md](scripting-api.md).

## What plugins are told

Plugins run before the commands are known, so `YIO_HELP`, `YIO_VERBOSE`, `YIO_CACHE`, and `YIO_REMOVE_CACHE` come from the command line as written: a built-in flag counts if its spelling appears anywhere before `--`. This differs from the real parse in one case. If a command defines its own flag with a built-in's spelling, such as its own `--cache`, plugins are still told the built-in was given.

Plugins run during `yio --cache` too. One that should not checks `YIO_CACHE` itself and stops.

Plugins are not told which command was named or what arguments it was given.

## An example

This directory has one. [`.yio/pre/0000_api.yio`](.yio/pre/0000_api.yio) reads the tables out of [scripting-api.md](scripting-api.md) and draws them as one table whenever `yio` is run in `docs/`:

```
$ API=dir yio
5 of 53 calls mention "dir"
Call               Group           Gives
-----------------  --------------  ---------------------------------------------------------------
(list-dir [path])  Reading files   The entries of a directory, sorted by name
(is-dir path)      Reading files   Whether the path is a directory
(make-dir path)    Changing files  Makes a directory, and any missing above it, and gives the path
(move from to)     Changing files  Moves a file or directory and gives the new path
(remove path)      Changing files  Removes a file or an empty directory and gives ()
```

The `API` environment variable narrows the table to the rows that mention its value. Because `env` and `read-file` record what they looked at, a cached result is reused only for the same filter and the same `scripting-api.md`.

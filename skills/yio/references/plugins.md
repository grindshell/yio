# Plugins and the cache

Scripts in a `.yio/` directory run whenever the `yio.md` beside it is used. Read [scripting.md](scripting.md) as well when writing one in the `.yio` language.

```
project/
  yio.md
  .yio/
    pre/        run before yio.md is read
    post/       run once the command tree is ready, before anything is listed or run
    cache.yio   written by `yio --cache`; keep it out of version control
```

## What they are for

- **Showing live information** above the command list or a command's output: a table of files and their status, counts, a progress panel. This is a `pre` plugin that evaluates to something to draw.
- **Answering a command with computed output.** The plugin looks at the command line and draws the answer; the command itself has a code block that does nothing.
- **Generating files**, including the `yio.md` itself. A `pre` plugin may create or rewrite the `yio.md` it sits beside, and runs even when that file does not exist yet.
- **Setup that must happen on every use**, in bash.

## Naming and order

Every file in `pre/` and `post/` must be named `NUMBER_NAME.bash` or `NUMBER_NAME.yio`, such as `0000_status.yio` or `10_fetch.bash`. Anything else in those directories is an error, so that a misnamed plugin is reported instead of never running. Names starting with a dot (`.gitkeep`) are ignored.

They run in order of the number, then by file name. `NAME` is what `--verbose` logs.

## When they run

- For every use of the `yio.md`: listing, `--help`, running a command, and `--cache`. Not for `--version` or `--remove-cache`.
- `pre` before the file is read; `post` after the whole tree is loaded. `post` is not "after the command": it runs before the command does.
- Each at most once per run.
- An embedded directory's plugins run when its `yio.md` is embedded, in that directory.
- A failing plugin stops the run, and the error names its file.

Because a plugin runs for every command in its directory, most start by deciding whether this run is theirs and stop quietly when it is not.

## Bash or `.yio`

| | `.bash` | `.yio` |
| --- | --- | --- |
| Good for | Anything: running programs, the network, the clock | Reading files and drawing what was found |
| Output | Whatever it prints | Its value, drawn by `yio` |
| Working directory | The directory of the `yio.md` | Relative paths resolve against the same directory |
| Fails when | It exits non-zero | It has an error |
| With a cache | Runs every time | Skipped while what it read is unchanged; its stored result is drawn |

Choose `.yio` when the plugin only reads files and shows something, because then the cache can skip its work and draw the stored result. Choose bash when it has to run a program (`git`, `cargo`, `curl`), which the `.yio` language cannot do by design.

## What a plugin is told

| | `.bash` | `.yio` |
| --- | --- | --- |
| The built-in flags | `$YIO_HELP`, `$YIO_VERBOSE`, `$YIO_CACHE`, `$YIO_REMOVE_CACHE`, `$YIO_VERSION` | `(env "YIO_HELP")` and so on, as the strings `"true"` and `"false"` |
| The rest of the command line | `"$@"` | `(args)`, a list of strings |

The rest of the command line is every word as typed, without the built-in flags: command names, their arguments, their own flags.

| Command line | The plugin is told |
| --- | --- |
| `yio` | nothing |
| `yio search file path` | `search` `file` `path` |
| `yio -V search --help file` | `search` `file` |
| `yio build web -j 8` | `build` `web` `-j` `8` |

Plugins run before the commands are known, so the words are not interpreted: a plugin is not told which are command names or whether the command exists.

In an embedded directory, a plugin is told only what follows the command its `yio.md` is embedded under. With `docs/yio.md` embedded under `## docs`, the plugins of `docs/` see `search file` for both `yio docs search file` from the root and `yio search file` inside `docs/`, and nothing for `yio build`.

Plugins run during `yio --cache` too. One that should not checks `YIO_CACHE` and stops.

## A plugin that shows a panel

`.yio/pre/0000_status.yio`, shown above the command list when `yio` is run with no command:

```lisp
-- Not above every command's output: only when no command was named. That
-- still includes `yio --help` and `yio --cache`, since the built-in flags
-- are not among the words. Check (env "YIO_HELP") or (env "YIO_CACHE")
-- against "true" as well to leave those out.
(if (or (not (is-empty (args))) (not (is-dir "notes")))
  return)

(var notes (filter (list-dir "notes") (fn (path) (= (extension path) "md"))))

-- `bar` needs a whole above zero, and there is nothing to show anyway.
(if (is-empty notes)
  return)

(fn is-done (path)
  (contains (read-file path) "status: done"))

(var done (filter notes is-done))

(box
  (fields
    (list "Notes" "Done" "Progress")
    (list (len notes) (len done) (bar (len done) (len notes) 12)))
  "Notes")
```

What a plugin draws is followed directly by the listing or the command's output. For a blank line between them, evaluate to a list that ends with an empty string: `(list (box ...) "")`.

## A plugin that answers a command

The `yio.md` declares the command so that it is listed, has help, and accepts the arguments:

````markdown
## find [...WORDS]

> List the notes that mention every word

```bash
# Answered by .yio/pre/0000_find.yio, which has run by now.
```
````

`.yio/pre/0000_find.yio`:

```lisp
(var words (args))

-- Stay out of the way of every other command, and of --help.
(if (or (not (= (first words) "find")) (= (env "YIO_HELP") "true"))
  return)

(var terms (rest words))

(fn mentions-all (path)
  (var text (read-file path))
  (is-empty (filter terms (fn (term) (not (contains text term))))))

(var hits (filter (list-dir "notes") mentions-all))

(if (is-empty hits)
  (return "No note mentions all of that."))

(col
  (col-def "Note" hits)
  (col-def "Size" (map hits file-size)))
```

The code block is needed: a command without one only prints its help. It does cost one start of bash on every use of the command, tens of milliseconds, and more through WSL. Where that matters, show the output on the bare `yio` listing instead, which starts no bash.

## A bash plugin

`.yio/pre/0010_branch.bash`:

```bash
# Runs in the directory of the yio.md, on every use of it.
[ "$YIO_CACHE" = true ] && exit 0
[ "$#" -eq 0 ] || exit 0
echo "On branch $(git branch --show-current)"
```

## The cache

`yio --cache` writes `.yio/cache.yio`. The file existing is what turns caching on.

- While it exists, `yio` reads the command tree from it and skips every `.yio` plugin whose inputs are unchanged, drawing its stored result instead. It keeps the file current by itself, and never uses a stale result.
- `yio --remove-cache` deletes it.
- What makes replaying safe is that every built-in of the `.yio` language records what it looked at: file contents, directory listings, environment variables, the command-line words. A plugin that calls `(args)` is replayed only for the same words.
- A `.yio` plugin that writes files is not rerun while what it wrote is still there. If its output is deleted or edited, it runs again.
- Bash plugins are never cached.
- With a cache, `yio` also warns about scripts that misuse a boolean flag (see SKILL.md).
- The cache describes one machine's files. Add `.yio/cache.yio` to `.gitignore`.
- The cache of the directory `yio` is started in covers embedded files and their plugins. An embedded directory's own cache is used only when `yio` is run from there.

Use the cache when `.yio` plugins do real work on every run, such as reading hundreds of files. With a few small files, or with no plugins, the saving is too small to notice next to starting the process, and the cache is not worth having.

## Checking a plugin

Run `yio` (or the command the plugin answers) and look at the output. A `.yio` error reads `line:column: message` with the plugin's file named. `yio -V` announces each plugin on stderr as it runs, and says which results came from the cache.

Styles (colour, bold) appear only when the output is a terminal. Captured output has none, which is expected. `CLICOLOR_FORCE=1` turns them on anyway, unless `NO_COLOR` is set, which turns them off and wins.

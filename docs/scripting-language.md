# Scripting language and cache: plan

A plan. It covers the `.yio` scripting language, the `.yio/cache.yio` file, and `.yio/pre/` and `.yio/post/` plugins, because the three share one file format and one notion of "dirty".

**Status**: phases 1 to 6 are implemented. The reader is described under "Syntax", the evaluator under "Semantics", the built-ins and what they record under "Built-ins and tracking", plugins under "Plugins", and the cache file, its dirtiness check, and its lifecycle under "The cache". What remains is phase 7: built-ins that write files, and tuning.

Statements marked **Given** come from the project's design. Everything else is a proposal. The decisions that need an answer before the affected phase starts are collected at the end.

## What caching is for

Measured on the current release build (Windows, 200 runs each):

| Case | Time per run |
| --- | --- |
| `yio --version`, no `yio.md` (process startup) | 6.4 ms |
| Load a 10-command `yio.md` (3 KB) | 6.4 ms |
| Load a 500-command `yio.md` (171 KB) | 8.8 ms |
| One `bash -c true` | 44 ms (Git Bash), 62 ms (WSL) |

Loading Markdown costs about 2.4 ms for 500 commands and nothing measurable for a normal file. A cached command tree therefore saves almost nothing by itself. One bash plugin costs seven to ten times a whole `yio` run.

So the cache earns its keep by **skipping plugin work**: a `.yio` plugin whose inputs have not changed is not run at all, and its stored result is replayed. That is what the language has to be designed around. The cached command tree is still worth building first, because it establishes the file format and the dirtiness check that plugin results reuse.

Measured again with the command tree cached (phase 2): the 500-command file goes from 8.8 ms to 7.6 ms per run, and the 10-command file does not change. That matches the prediction.

Measured with script results cached (phase 6), using the design sketch's plugin: it lists a directory of lore files of about 1 KB each, reads every one for its status line, and draws a three-column table. Time for one `yio` run, table included:

| Lore files | No cache | Cached |
| --- | --- | --- |
| 20 | 10.9 ms | 7.8 ms |
| 200 | 35.9 ms | 12.6 ms |
| 1000 | 136 ms | 34 ms |

A cached run does not start the evaluator or read any lore file. What it still pays for is one `stat` per input, about 26 µs each on this Windows machine, plus drawing the table. Process startup is 6.3 ms of every figure.

## The cache

### Units

The cache holds independent units. Each records the inputs it was computed from, so one changing file invalidates only the units that read it.

| Unit | Computed from | Stored |
| --- | --- | --- |
| Command tree | `yio.md` and every file it embeds | The `Command` tree |
| Script result (one per `.yio` plugin) | The script's source and everything its built-ins read | The value the script evaluated to |

On each run, `yio` checks every unit's inputs. A clean unit is decoded and used. A dirty unit is recomputed the normal way.

A script's inputs always include its own file, so editing a plugin reruns it. An input that another already covers is dropped before storing: a file that was read needs no separate record of its size or of being a file.

Bash plugins have no unit. They can read anything, so they run every time.

### Dirtiness

An input is one thing a result depended on, recorded narrowly so that only a change that could alter the result counts. Implemented in `src/input.rs`.

| Input | Recorded | Changed when |
| --- | --- | --- |
| A file's contents | Size, modification time, content hash | The content differs |
| A directory's listing | Modification time, hash of the sorted names | A name is added, removed, or renamed. Editing a file inside does not count. |
| What a path is | File, directory, something else, or missing | It appears, disappears, or changes type |
| A file's size | The size | The size differs. Content of the same size does not count. |
| An environment variable | Hash of the value, or that it was unset | The value differs |

The last three are settled by one `stat` or one environment lookup. Files and listings follow the steps below, which are written for a file; a listing is the same with the directory re-listed instead of the file re-read.

Each unit also stores an **as-of time**, taken at the start of the run that recorded its inputs, before any of them was read.

Checking an input:

1. `stat` it. A missing file or a different size is dirty.
2. If the modification time matches and is more than two seconds older than the as-of time, it is clean. No file is read. This is the normal case and costs one system call per input.
3. Otherwise the timestamp proves nothing, so read the file and compare the hash. This covers two cases. A file modified within two seconds before the recording could have been changed again afterwards without its timestamp moving on (Git's index has the same problem and the same answer). And a file whose time changed but whose content did not, after `touch` or a checkout, should not invalidate anything.

Two seconds is the timestamp resolution of FAT, the coarsest file system worth supporting.

When step 3 finds the content unchanged and the file is by now more than two seconds old, the cache is rewritten with the new modification time and a new as-of time, so later runs are back on the `stat`-only path.

The hash is FNV-1a, about six lines of code and no dependency. It detects change; it is not a security boundary. The files are read anyway when the unit is first computed, so hashing them then is free.

The limit of this scheme: a file rewritten with different content of the same size, whose modification time is then set back to exactly what it was, is not noticed. That takes deliberate effort.

The whole cache is also discarded if it was written by a different `yio` version or cache format version, or if it is damaged.

### File format

**Given**: `cache.yio` is a single `(cache <raw bytes>)` expression, sharing the scripting language's format where possible, and the bytes are a custom encoding optimized for interpretation speed.

Raw bytes cannot sit inside a text format unless the reader knows where they end, since they may contain `)`. The bytes literal, usable in any `.yio` file:

```
#<length in decimal>:<exactly that many raw bytes>
```

so a cache file is `(cache #5120:` followed by 5120 bytes and `)`. The cache loader does not run the general reader: it checks the fixed prefix, reads the length, and decodes the payload in place.

Payload, all integers fixed-width little-endian (no varint branching), strings as a `u32` length plus UTF-8:

```
u8   format version (currently 2)
str  yio version
u32  unit count
unit:
  u8   kind (command tree, script result)
  str  source path (yio.md, or the plugin's path)
  u64  as-of time, ns since the Unix epoch
  u32  input count
  input: u8 kind, then that kind's fields
  u32  body length
  body
```

Each unit has its own as-of time, because units are recomputed on different runs.

The body length lets a dirty unit be skipped without decoding it. A command tree body is the `Command` records in pre-order, decoded in one pass straight into the structs the CLI already uses. A script result body is a tagged value (see "Values").

"Compressed" means a compact binary encoding. General-purpose compression (deflate and the like) would need a dependency or a hand-written codec and would make loading slower, which works against the stated goal.

A file that is damaged or truncated is detected rather than trusted: every read of the payload is bounds-checked, and nesting is limited to 256 levels.

### Lifecycle

The cache file existing is what turns caching on.

- `yio --cache` computes everything again, ignoring any existing cache: it loads the Markdown and runs every plugin, then writes `.yio/cache.yio` and exits. It does not list or run a command.
- `yio --remove-cache` deletes the file and exits. It works even when `yio.md` fails to load.
- While the file exists, every run uses each unit that is clean and recomputes each one that is not, then rewrites the file if anything changed, so the cache stays current without re-running `--cache`. A stale result is never used. A file that is damaged or from another version is recomputed whole.
- A unit the run did not need is dropped, which is how the result of a deleted plugin goes away.
- A run that does not run plugins (`--version`) sees only part of what the cache holds, so it leaves the file alone.
- That rewrite is best effort. If the directory cannot be written to, the command still runs.
- The file is replaced in one step (written beside itself, then renamed), so a concurrent `yio` never reads half of it.

A directory's cache covers the tree it loads, including embedded files, and the `.yio` plugins of every directory involved. An embedded directory's own `cache.yio` is used only when `yio` is run from that directory.

The cache describes one machine's files by modification time, so it should not be committed: add `.yio/cache.yio` to `.gitignore`.

## The language

### Syntax

Implemented in `src/script/reader.rs`, which turns the bytes of a file into forms, each tagged with its line and column.

| Form | Written as | Notes |
| --- | --- | --- |
| List | `(a b c)` | `()` is the empty value. Lists nest at most 256 deep. |
| Symbol | `list-dir`, `is-empty?`, `<=` | ASCII letters, digits, and `-_?!*/%<>=+`. |
| Integer | `42`, `-7` | 64-bit signed. A token that starts like a number must be one: `12ab` is an error. |
| Boolean | `true`, `false` | |
| String | `"lore"` | UTF-8. Escapes: `\n`, `\t`, `\r`, `\"`, `\\`. May span lines. |
| Bytes | `#3:abc` | A decimal length, a colon, then exactly that many raw bytes of any value. |
| Comment | `-- text` | From `--` at the start of a token to the end of the line. `a--b` is a symbol. |

Whitespace separates tokens and is otherwise ignored. Outside strings, bytes, and comments, the file must be ASCII. Errors read `line:column: message`, with columns counted in characters.

The nesting limit exists because everything that walks forms afterwards recurses, so a file nested without bound would overflow the stack instead of reporting an error.

### Given by the design sketch

- S-expressions; `--` starts a comment that runs to the end of the line.
- `(var name expr)` declares a variable.
- `(if cond then)`.
- A bare `return` leaves the script or function early.
- `(fn name (args) body...)` defines a function; `(fn (args) body...)` is anonymous and can be called immediately. The last expression is the return value.
- `()` is the empty value.
- Built-ins `list-dir`, `is-empty`, `map` (lists only), and the UI built-ins `col` and `col-def`.

### Semantics

Implemented in `src/script/eval.rs`. A list is a call unless its first element is a keyword, and everything is an expression.

| Form | Meaning |
| --- | --- |
| `(var name value)` | Declares a variable in the current scope. Declaring the same name twice in one scope is an error. |
| `(set name value)` | Changes the nearest variable with that name. It must already exist. |
| `(if cond then)`, `(if cond then else)` | Evaluates only the branch taken. Without an `else`, a false condition gives `()`. |
| `(do expr...)` | Evaluates each expression and gives the last. It has its own scope. |
| `(fn name (params...) body...)` | Defines a function and binds it to `name`. |
| `(fn (params...) body...)` | Evaluates to an anonymous function. |
| `return`, `(return)`, `(return value)` | Leaves the innermost function, or the script when used outside one. |
| `(and a b ...)`, `(or a b ...)` | Stop at the first false or first true value and give it; otherwise give the last value. |
| `(f args...)` | Evaluates `f`, then the arguments left to right, then calls. |

- **Keywords** are `var`, `set`, `if`, `do`, `fn`, `return`, `and`, `or`. They cannot name a variable. A script may reuse a built-in's name for its own variable, but cannot `set` a built-in.
- **The empty value**: `()` is the empty list. There is no separate "nothing". It is what `var`, `set`, a named `fn`, an empty body, and an `if` with no `else` evaluate to.
- **Truthiness**: only `false` and `()` are false. `0` and `""` are true.
- **Scope** is lexical. A function sees, and can change, the variables where it was defined, and two functions from the same place share them. `do`, function bodies, and the script itself each have a scope.
- **Functions** check the number of arguments. Named functions can call themselves and each other.
- **Integers** are 64-bit. Overflow and division by zero are errors, not wraparound. `/` truncates toward zero.
- **No floats**: file work does not need them and they complicate stable encoding.
- **No** macros, `eval`, imports, or loop forms. Iteration is `map`, `filter`, and recursion.
- **Errors** stop the script and report `line:column: message`, pointing at the call that failed.

Evaluation nests at most 2000 deep, counting each nested expression and each function call, so runaway recursion is an error rather than a crash. That allows several hundred recursive calls, enough to recurse over a list of 500 items.

Evaluation recurses on the native stack, and the main thread's (1 MiB on Windows) is too small for a useful limit, so every script runs on its own thread with a 64 MiB stack, of which only the used part is committed. Measured cost of one level:

| Path | Release | Debug |
| --- | --- | --- |
| A function calling itself | 0.65 KiB | 3.6 KiB |
| A function calling itself through `map` | 1.4 KiB | 5.7 KiB |

At the limit that is at most 2.7 MiB in a release build and 11.2 MiB in a debug build. A test drives each path to the limit, so frames growing past the stack shows up as a failing test run, not as a crash in use.

A script's values cannot leave that thread. `script::run` therefore takes a function that receives the outcome on the script's thread and reduces it to something plain, such as rendered text or encoded bytes.

Not guarded: a value nested hundreds of thousands deep, built without recursion by wrapping a variable in a new list on every pass of a `map`. Freeing it recurses once per level.

### A script is a value

A script evaluates to the value of its last top-level expression (or the value given to `return`). `yio` then renders that value: a table is drawn, a string is printed, a list is rendered item by item, `()` prints nothing.

UI built-ins therefore build values rather than print. `(col (col-def "File" names) (col-def "Size" sizes))` evaluates to a table value; nothing reaches the terminal until the script finishes. This is what makes replay trivial: the cache stores the value, and a clean run renders it without evaluating anything.

Reading of the sketch: `col` takes column definitions and produces the table, and each `col-def` is a header string plus a list of cell values.

### Values

`()`, booleans, integers, strings, bytes, lists, functions, columns, and tables. All except functions have a binary encoding of one tag byte plus payload (`src/script/encode.rs`), which is how a script's value is kept in the cache. A script whose result contains a function is an error.

A value nested more than 256 levels deep is not stored, so its script simply runs every time.

Lists and strings are never changed in place. `append` and the like give a new value, so a value handed to a function cannot be altered behind the caller's back.

### Built-ins and tracking

Every built-in that looks at the outside world records what it looked at. That record is the unit's input list. A script is then a pure function of its recorded inputs, which is the property that makes replay correct.

Implemented in `src/script/builtins.rs`. Running a script gives its value together with the list of inputs these recorded.

**The outside world.** Each call records one input.

| Call | Gives | Records |
| --- | --- | --- |
| `(list-dir)`, `(list-dir path)` | The entries, sorted by name. Each is spelled ready to pass on: `(list-dir "lore")` gives `"lore/a.md"`. With no argument, bare names from the script's own directory. | The listing |
| `(read-file path)` | The contents, which must be UTF-8 | The contents |
| `(file-size path)` | The size in bytes | The size |
| `(is-file path)`, `(is-dir path)` | A boolean. A missing path is neither. | What the path is |
| `(env name)` | The value, or `()` if unset | The value |

A missing file or directory is an error for `list-dir`, `read-file`, and `file-size`. Relative paths resolve against the script's base directory, which will be the one holding its `.yio/` directory. Scripts may read anywhere, including absolute paths and `..`.

**Paths**, as text only: nothing is looked up.

| Call | Gives |
| --- | --- |
| `(path-join part...)` | The parts joined with `/`. A part starting with `/` replaces what came before; empty parts are skipped. |
| `(base-name path)` | The last component: `a.md` for `lore/a.md` |
| `(extension path)` | What follows the last dot of the base name, without the dot. Empty for `Makefile` and `.gitignore`. |

**Text**

| Call | Gives |
| --- | --- |
| `(split text separator)` | A list of the pieces |
| `(join list separator)` | The strings of a list, joined |
| `(lines text)` | The lines, without their line endings |
| `(trim text)` | Without whitespace at either end |
| `(starts-with text prefix)`, `(ends-with text suffix)` | A boolean |
| `(contains text part)`, `(contains list item)` | A boolean |
| `(replace text from to)` | With every occurrence replaced |
| `(str value...)` | The values as text, run together. Strings as they are, `()` as nothing, anything else as a script would write it. |

**Lists**

| Call | Gives |
| --- | --- |
| `(list item...)` | A list |
| `(is-empty value)` | Whether a list or string has nothing in it |
| `(len value)` | Items in a list, characters in a string, or bytes |
| `(first list)` | The first item, or `()` for an empty list |
| `(rest list)` | Everything after the first item |
| `(nth list index)` | The item at an index, from zero. Out of range is an error. |
| `(append list item...)` | A new list with the items added |
| `(map list function)` | The results of calling the function on each item |
| `(filter list function)` | The items for which the function gives a true value |

`map` and `filter` take the list first, as in the design sketch, and work only on lists. The function can be a script's own or a built-in: `(map files file-size)`.

**Tables**

| Call | Gives |
| --- | --- |
| `(col-def header cells)` | A column: a header string and a list of cells |
| `(col column...)` | A table of those columns, for `yio` to draw |

**Arithmetic and comparison**: `+ - * / %`, `= < > <= >=`, `not`. `=` compares any two values structurally, and values of different kinds are simply unequal. The ordering operators take two integers or two strings.

Deliberately absent: the clock, randomness, the network, and running other programs. Any of them makes a result impossible to replay. If one is ever needed it should mark the unit as never cached.

Writing files (`write-file`, `make-dir`, `copy`, `move`, `remove`) is a later phase. A write is recorded as an output, and a unit is also dirty when one of its outputs is missing or changed, the way a build system treats targets.

## Plugins

**Given**: `.yio/pre/` scripts run before the sibling `yio.md` is processed and `.yio/post/` scripts after. Names are `NUMBER_NAME.bash` or `NUMBER_NAME.yio`; `NUMBER` is a `u64`. They run at cache time too unless they check `YIO_CACHE`.

Implemented in `src/plugin.rs`.

### Which files are plugins

Every file in `.yio/pre/` and `.yio/post/` must be named `NUMBER_NAME.bash` or `NUMBER_NAME.yio`. Anything else is an error, so a misnamed plugin is reported instead of silently never running. Names that start with a dot, such as `.gitkeep`, are left alone.

They run in order of `NUMBER`, then by file name. `0002_a` and `2_b` have the same number.

### When they run

| Stage | Runs |
| --- | --- |
| `pre` | Before the `yio.md` beside it is read. With a cache, before the cached tree is checked, because a `pre` plugin may rewrite the files that tree was built from. |
| `post` | Once the whole command tree is ready and before anything is listed or run. Directories go in the order their `yio.md` was reached, the starting directory first. |

- A plugin runs at most once per `yio` run.
- An embedded directory's plugins run when its `yio.md` is embedded: its `pre` plugins just before that file is read.
- Plugins run for listing, `--help`, running a command, and `--cache`. They do not run when `--version` or `--remove-cache` is on the command line, since neither uses the `yio.md`.
- A `pre` plugin may create or rewrite the `yio.md` it sits beside. It runs even when that file does not exist yet.
- A failing plugin stops the run. The error names the plugin's file.
- With `--verbose`, each plugin is announced on stderr by name before it runs.

`post` running after loading rather than after the chosen command is a decision: it keeps plugins about the `yio.md`, and the command's exit code stays `yio`'s.

### The two kinds

| | `.bash` | `.yio` |
| --- | --- | --- |
| Run by | bash, as a script file | The evaluator |
| Working directory | The directory of the `yio.md` | Relative paths resolve against the same directory |
| Output | Whatever it prints | Its value, rendered (see below) |
| Sees | The `YIO_*` variables in its environment | The same through `(env "YIO_CACHE")` and the like |
| Fails when | It exits non-zero | It has an error, or its value contains a function |
| With a cache | Runs every time | Not run while its inputs are unchanged: its stored value is rendered instead |

Bash plugins are opaque, so they can never be replayed. That is the reason to write a plugin in the scripting language.

### What plugins are told

Plugins run before the commands are known, so `YIO_HELP`, `YIO_VERBOSE`, `YIO_CACHE`, and `YIO_REMOVE_CACHE` come from the command line as written: a built-in flag counts if its spelling appears anywhere before `--`. This differs from the real parse in one case. If a command defines its own flag with a built-in's spelling, such as its own `--cache`, plugins are still told the built-in was given.

A plugin that should not run during `yio --cache` checks `YIO_CACHE` itself and stops.

A `YIO_*` variable a script reads is one of its inputs, like any environment variable. A value computed during `yio --cache` by a script that read `YIO_CACHE` is therefore not reused by a normal run, where that variable differs: the script runs once more and that result is stored. A script that reads no such variable is replayed from the first normal run on.

With `--verbose`, a replayed plugin is announced as `from the cache`.

### Rendering a value

Implemented in `src/script/render.rs`.

| Value | Rendered as |
| --- | --- |
| `()` | Nothing |
| A string | The text, on its own line |
| A table | A row of headers, a rule, then one row per cell, in columns padded to the widest entry |
| A column | A table of that one column |
| A list | Each item in turn, so a script can give a heading, a table, and a note |
| An integer, boolean, or bytes | As a script would write it |
| A function | An error |

In a table cell, `()` is blank and line breaks become spaces. Columns of different lengths are padded with blanks. Widths count characters, so wide characters such as CJK can misalign.

## Phases

Each phase ends with tests and its doc updated. Caching starts producing a file at the end of phase 2 and starts saving real time at the end of phase 6.

| # | Phase | Delivers |
| --- | --- | --- |
| 1 | Reader (done) | Bytes in, forms out, with positions for errors. Comments, strings, integers, symbols, lists, `()`, and the bytes literal. `src/script/reader.rs`. |
| 2 | Cache file and command tree (done) | Encode and decode the payload, fingerprints, the dirtiness check, `--cache`, `--remove-cache`, loading from a clean cache. `src/cache.rs`. |
| 3 | Evaluator (done) | Values, scopes, the special forms, functions and closures, errors. No built-ins that touch the outside world. `src/script/eval.rs`. |
| 4 | Built-ins and tracking (done) | The first set above, with input recording. `src/script/builtins.rs`. |
| 5 | Plugins (done) | Discovery, ordering, the bash and `.yio` runners, rendering values, embedded directories. `src/plugin.rs`. |
| 6 | Script result caching (done) | Script result units in the cache, replay on clean runs. The docs-table example runs end to end and is skipped when nothing changed. |
| 7 | Writes and tuning | Write built-ins with output tracking; benchmark with realistic plugin sets before optimizing the encoding further. |

## Decisions needed

Decided: the bytes literal is `#<length>:<bytes>`; "compressed" means a compact encoding; a dirty cache refreshes itself; `set` reassignment is allowed; a script's output is its final value; scripts may read files anywhere; `.bash` plugins run on every access; `post/` runs after `yio.md` is loaded.

| # | Question | Proposed | Blocks |
| --- | --- | --- | --- |
| 8 | May scripts write outside the directory tree of their `yio.md`? | No, write only inside | Phase 7 |

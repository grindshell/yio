# Scripting language and cache: plan

A plan. It covers the `.yio` scripting language, the `.yio/cache.yio` file, and `.yio/pre/` and `.yio/post/` plugins, because the three share one file format and one notion of "dirty".

**Status**: phases 1 to 3 are implemented. The reader is described under "Syntax", the evaluator under "Semantics", and the cache file, its dirtiness check, and its lifecycle under "The cache", which holds the command tree only. Built-ins that read files, plugins, and script result caching do not exist yet, so `yio` itself does not run scripts: outside tests, nothing calls the reader or the evaluator until phase 5.

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

Measured again with the command tree cached (phase 2): the 500-command file goes from 8.8 ms to 7.6 ms per run, and the 10-command file does not change. That matches the prediction, and is the whole benefit until script results are cached.

## The cache

### Units

The cache holds independent units. Each records the inputs it was computed from, so one changing file invalidates only the units that read it.

| Unit | Computed from | Stored |
| --- | --- | --- |
| Command tree | `yio.md` and every file it embeds | The `Command` tree |
| Script result (one per `.yio` plugin) | The script's source and everything its built-ins read | The value the script evaluated to |

On each run, `yio` checks every unit's inputs. A clean unit is decoded and used. A dirty unit is recomputed the normal way.

Only the command tree unit exists so far.

### Dirtiness

An input is one of: a file's contents, a directory's listing, the absence of a path, or an environment variable's value. Only file inputs exist so far. Each stores a fingerprint of three things: size, modification time in nanoseconds, and a 64-bit hash of the contents (for a directory, it will be a hash of the sorted entry names).

The cache also stores an **as-of time**, taken at the start of the run that recorded the inputs, before any of them was read.

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
u8   format version
str  yio version
u64  as-of time, ns since the Unix epoch
u32  unit count
unit:
  u8   kind (command tree, script result)
  str  source path
  u32  input count
  input: u8 kind, str path or name, u64 size, u64 mtime ns, u64 hash
  u32  body length
  body
```

The body length lets a dirty unit be skipped without decoding it. A command tree body is the `Command` records in pre-order, decoded in one pass straight into the structs the CLI already uses. A script result body is a tagged value (see "Values").

"Compressed" means a compact binary encoding. General-purpose compression (deflate and the like) would need a dependency or a hand-written codec and would make loading slower, which works against the stated goal.

A file that is damaged or truncated is detected rather than trusted: every read of the payload is bounds-checked, and nesting is limited to 256 levels.

### Lifecycle

The cache file existing is what turns caching on.

- `yio --cache` loads everything from the Markdown, ignoring any existing cache, writes `.yio/cache.yio`, and exits. It does not list or run anything.
- `yio --remove-cache` deletes the file and exits. It works even when `yio.md` fails to load.
- While the file exists, every run uses it if it is clean. If it is dirty, damaged, or from another version, the run loads from the Markdown as usual and rewrites the file, so the cache stays current without re-running `--cache`. A stale result is never used.
- That rewrite is best effort. If the directory cannot be written to, the command still runs.
- The file is replaced in one step (written beside itself, then renamed), so a concurrent `yio` never reads half of it.

A directory's cache covers the tree it loads, including embedded files. An embedded directory's own `cache.yio` is used only when `yio` is run from that directory.

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

Evaluation nests at most 200 deep, counting each nested expression and each function call, so runaway recursion is an error rather than a crash. The limit is sized for the smallest stack `yio` runs on, the 1 MiB main thread on Windows: one level costs up to about 650 bytes in a release build and 3.6 KiB in a debug build. That allows roughly 60 to 200 recursive calls depending on how much each one nests, which is enough for walking directories but not for recursing over a long list; `map` and `filter` are for that. Raising it means running scripts on a thread with a larger stack, which is easiest to do when plugins are wired in (phase 5).

The pure built-ins exist already: `+ - * / %`, `= < > <= >=`, `not`, `list`, and `is-empty`. `=` compares any two values structurally, and values of different kinds are simply unequal. The ordering operators take two integers or two strings.

### A script is a value

A script evaluates to the value of its last top-level expression (or the value given to `return`). `yio` then renders that value: a table is drawn, a string is printed, a list is rendered item by item, `()` prints nothing.

UI built-ins therefore build values rather than print. `(col (col-def "File" names) (col-def "Size" sizes))` evaluates to a table value; nothing reaches the terminal until the script finishes. This is what makes replay trivial: the cache stores the value, and a clean run renders it without evaluating anything.

Reading of the sketch: `col` takes column definitions and produces the table, and each `col-def` is a header string plus a list of cell values.

### Values

`()`, booleans, integers, strings, bytes, lists, functions, and view nodes (table, column). All except functions have a binary encoding of one tag byte plus payload, the same encoding the cache uses. A script whose result contains a function is an error.

### Built-ins and tracking

Every built-in that looks at the outside world records what it looked at. That record is the unit's input list. A script is then a pure function of its recorded inputs, which is the property that makes replay correct.

| Group | First set | Recorded input |
| --- | --- | --- |
| Reading files | `list-dir`, `read-file`, `file-size`, `is-file`, `is-dir` | The file, the directory listing, or the path's absence |
| Environment | `env` | The variable's value |
| Paths | `path-join`, `base-name`, `extension` | None |
| Text | `split`, `join`, `lines`, `trim`, `starts-with`, `ends-with`, `contains`, `replace`, `len`, `str` | None |
| Lists | `list`, `map`, `filter`, `is-empty`, `len`, `first`, `rest`, `nth`, `append` | None |
| UI | `col`, `col-def` | None |

Relative paths resolve against the directory that holds the `.yio/` directory, matching where that directory's scripts run.

Deliberately absent: the clock, randomness, the network, and running other programs. Any of them makes a result impossible to replay. If one is ever needed it should mark the unit as never cached.

Writing files (`write-file`, `make-dir`, `copy`, `move`, `remove`) is a later phase. A write is recorded as an output, and a unit is also dirty when one of its outputs is missing or changed, the way a build system treats targets.

## Plugins

**Given**: `.yio/pre/` scripts run before the sibling `yio.md` is processed and `.yio/post/` scripts after. Names are `NUMBER_NAME.bash` or `NUMBER_NAME.yio`; `NUMBER` is a `u64`. They run at cache time too unless they check `YIO_CACHE`.

Proposed:

- Order is by `NUMBER`, then by name. A file that does not match the pattern is an error, not skipped.
- A `.bash` plugin runs through the existing runner, in the `yio.md`'s directory, with the `YIO_*` variables.
- A `.yio` plugin is evaluated and its value rendered to stdout.
- A failing plugin stops the run and names the plugin.
- Plugins run for listing, `--help`, running a command, and `--cache`. They do not run for `--version`.
- An embedded directory's plugins run when its `yio.md` is embedded.
- Bash plugins are opaque, so they cannot be replayed: with a valid cache they still run every time, and only `.yio` plugins are skipped. This is the reason to write a plugin in the scripting language.

## Phases

Each phase ends with tests and its doc updated. Caching starts producing a file at the end of phase 2 and starts saving real time at the end of phase 6.

| # | Phase | Delivers |
| --- | --- | --- |
| 1 | Reader (done) | Bytes in, forms out, with positions for errors. Comments, strings, integers, symbols, lists, `()`, and the bytes literal. `src/script/reader.rs`. |
| 2 | Cache file and command tree (done) | Encode and decode the payload, fingerprints, the dirtiness check, `--cache`, `--remove-cache`, loading from a clean cache. `src/cache.rs`. |
| 3 | Evaluator (done) | Values, scopes, the special forms, functions and closures, errors. No built-ins that touch the outside world. `src/script/eval.rs`. |
| 4 | Built-ins and tracking | The first set above, with input recording. `src/script/builtins.rs`. |
| 5 | Plugins | Discovery, ordering, the bash and `.yio` runners, rendering values, embedded directories. `src/plugin.rs`. |
| 6 | Script result caching | Script result units in the cache, replay on clean runs. The docs-table example runs end to end and is skipped when nothing changed. |
| 7 | Writes and tuning | Write built-ins with output tracking; benchmark with realistic plugin sets before optimizing the encoding further. |

## Decisions needed

Decided: the bytes literal is `#<length>:<bytes>`; "compressed" means a compact encoding; a dirty cache refreshes itself; `set` reassignment is allowed; a script's output is its final value.

| # | Question | Proposed | Blocks |
| --- | --- | --- | --- |
| 6 | With a valid cache, do `.bash` plugins still run on every access? | Yes | Phase 5 |
| 7 | Does `post/` run after `yio.md` is loaded, or after the chosen command finishes? | After it is loaded | Phase 5 |
| 8 | May scripts read or write outside the directory tree of their `yio.md`? | Read anywhere, write only inside | Phases 4 and 7 |

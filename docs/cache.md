# The cache

`.yio/cache.yio` holds results `yio` would otherwise compute again on every run: the command tree, and the value of each `.yio` plugin. Implemented in `src/cache.rs`, with the dirtiness check in `src/input.rs`.

## What it is for

Measured on a release build (Windows, 200 runs each):

| Case | Time per run |
| --- | --- |
| `yio --version`, no `yio.md` (process startup) | 6.4 ms |
| Load a 10-command `yio.md` (3 KB) | 6.4 ms |
| Load a 500-command `yio.md` (171 KB) | 8.8 ms |
| One `bash -c true` | 44 ms (Git Bash), 62 ms (WSL) |

Loading Markdown costs about 2.4 ms for 500 commands and nothing measurable for a normal file. Caching the command tree alone saves almost nothing: the 500-command file goes from 8.8 ms to 7.6 ms.

The cache earns its keep by **skipping plugin work**. A `.yio` plugin whose inputs have not changed is not run at all, and its stored value is drawn instead. Measured with a plugin that lists a directory of lore files of about 1 KB each, reads every one for its status line, and draws a three-column table:

| Case | No cache | Cached |
| --- | --- | --- |
| 20 lore files | 10.4 ms | 7.5 ms |
| 200 lore files | 33.0 ms | 8.4 ms |
| 1000 lore files | 137.6 ms | 14.7 ms |
| A project: 4 `yio.md` files, 6 `.yio` plugins, 300 notes | 48.9 ms | 10.6 ms |

Process startup is about 6.5 ms of every figure. The project case embeds three directories, each with a `pre` plugin that draws a table of its notes and a `post` plugin that writes an index file. A cached run does not start the evaluator or read any of those files.

## Lifecycle

The cache file existing is what turns caching on.

- `yio --cache` computes everything again, ignoring any existing cache: it loads the Markdown and runs every plugin, then writes `.yio/cache.yio` and exits. It does not list or run a command.
- `yio --remove-cache` deletes the file and exits. If that leaves `.yio/` empty, the directory goes too, since `--cache` made it where there was none. A `.yio/` that holds plugins stays. It works even when `yio.md` fails to load.
- While the file exists, every run uses each unit that is clean and recomputes each one that is not, then rewrites the file if anything changed. The cache stays current without `--cache` being run again, and a stale result is never used.
- A unit the run did not need is dropped, which is how the result of a deleted plugin goes away.
- A run that does not run plugins (`--version`) sees only part of what the cache holds, so it leaves the file alone.
- The rewrite is best effort. If the directory cannot be written to, the command still runs.
- The file is replaced in one step (written beside itself, then renamed), so a concurrent `yio` never reads half of it.
- A file that is damaged, or written by a different `yio` version or cache format version, is discarded and recomputed whole.

A directory's cache covers the tree it loads, including embedded files, and the `.yio` plugins of every directory involved. An embedded directory's own `cache.yio` is used only when `yio` is run from that directory.

The cache describes one machine's files by modification time, so it should not be committed: add `.yio/cache.yio` to `.gitignore`.

## Units

The cache holds independent units. Each records the inputs it was computed from, so one changing file invalidates only the units that read it.

| Unit | Computed from | Stored |
| --- | --- | --- |
| Command tree | `yio.md` and every file it embeds | The `Command` tree |
| Script result (one per `.yio` plugin) | The script's source and everything its built-ins looked at | The value the script evaluated to |

A script's inputs always include its own file, so editing a plugin reruns it. An input that another already covers is dropped before storing: a file that was read needs no separate record of its size or of being a file.

Bash plugins have no unit. They can read anything, so they run every time.

## Dirtiness

An input is one thing a result depended on, recorded narrowly so that only a change that could alter the result counts.

| Input | Recorded | Changed when |
| --- | --- | --- |
| A file's contents | Size, modification time, content hash | The content differs |
| A directory's listing | Modification time, hash of the sorted names | A name is added, removed, or renamed. Editing a file inside does not count. |
| What a path is | File, directory, something else, or missing | It appears, disappears, or changes type |
| A file's size | The size | The size differs. Content of the same size does not count. |
| An environment variable | Hash of the value, or that it was unset | The value differs |
| The arguments a plugin was told | Hash of the words | Any word differs, or their number or order |

What a path is and a file's size are settled by one `stat`, and the last two without touching the disk. Files and listings follow the steps below, which are written for a file; a listing is the same with the directory re-listed instead of the file re-read.

Each unit also stores an **as-of time**, taken at the start of the run that recorded its inputs, before any of them was read.

Checking an input:

1. `stat` it. A missing file or a different size is dirty.
2. If the modification time matches and is more than two seconds older than the as-of time, it is clean. No file is read. This is the normal case and costs one system call per input.
3. Otherwise the timestamp proves nothing, so read the file and compare the hash. This covers two cases. A file modified within two seconds before the recording could have been changed again afterwards without its timestamp moving on (Git's index has the same problem and the same answer). And a file whose time changed but whose content did not, after `touch` or a checkout, should not invalidate anything.

Two seconds is the timestamp resolution of FAT, the coarsest file system worth supporting.

When step 3 finds the content unchanged and the file is by now more than two seconds old, the cache is rewritten with the new modification time and a new as-of time, so later runs are back on the `stat`-only path.

The hash is FNV-1a, about six lines of code and no dependency. It detects change; it is not a security boundary. The files are read anyway when the unit is first computed, so hashing them then is free.

The limit of this scheme: a file rewritten with different content of the same size, whose modification time is then set back to exactly what it was, is not noticed. That takes deliberate effort.

### Scripts that change files

A change a script makes is recorded as the state it left behind: a file it wrote is recorded with the contents it wrote. The script's result is reused only while that state still holds. So a script that has not been rerun has not had its changes redone, and does not need to: what it made is still there. If a file it wrote is deleted or edited by hand, the result is out of date, the script runs again, and the file is put back. This is how a build tool treats its targets.

A script that changes something it looked at earlier in the same run invalidates its own observation, and so runs once more before it settles:

```lisp
(if (is-file "config.txt")
  "kept"
  (do (write-file "config.txt" "default") "created"))
```

The first run sees no file and makes one. Its record says "there was no `config.txt`", which is no longer true, so the next run is not replayed: it sees the file and records that instead. From then on it is replayed, until the file is deleted. A script that changes its own input on every run, such as one that appends to a file it reads, never settles and runs every time, which is what such a script needs.

### Flags and arguments a script reads

The command-line words a script reads with `(args)` are one of its inputs, so its result is reused only for the same words. The cache keeps one result per plugin: a different command line replaces it.

A `YIO_*` variable a script reads is an input too, like any environment variable. A value computed during `yio --cache` by a script that read `YIO_CACHE` is therefore not reused by a normal run, where that variable differs: the script runs once more and that result is stored. A script that reads no such variable is replayed from the first normal run on.

## File format

`cache.yio` is a single `(cache <raw bytes>)` expression in the scripting language's syntax. The bytes are a custom binary encoding built for decoding speed, not for size or readability.

Raw bytes cannot sit inside a text format unless the reader knows where they end, since they may contain `)`. Hence the bytes literal, usable in any `.yio` file:

```
#<length in decimal>:<exactly that many raw bytes>
```

so a cache file is `(cache #5120:` followed by 5120 bytes and `)`. The cache loader does not run the general reader: it checks the fixed prefix, reads the length, and decodes the payload in place.

Payload, with all integers fixed-width little-endian (no varint branching) and strings as a `u32` length plus UTF-8 (`src/bytes.rs`):

```
u8   format version (currently 7)
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

Each unit has its own as-of time, because units are recomputed on different runs. The body length lets a dirty unit be skipped without decoding it.

A command tree body is the `Command` records in pre-order, decoded in one pass straight into the structs the CLI already uses. Each script is stored with the line of its `yio.md` it starts on, so that warnings about it can name the line. A script result body is a value as one tag byte plus payload (`src/script/encode.rs`). A value nested more than 256 levels deep is not stored, so its script simply runs every time.

The encoding is compact but not compressed. General-purpose compression (deflate and the like) would need a dependency or a hand-written codec and would make loading slower.

Every read of the payload is bounds-checked and nesting is limited to 256 levels, so a damaged or truncated file is detected rather than trusted.

## Where a cached run spends its time

Measured on the project case above, by comparing runs that do more and less:

| Part | Time |
| --- | --- |
| Starting the process | 6.5 ms |
| Reading and parsing the cache file (27 KB), checking and decoding the command tree | 0.2 ms |
| Checking the plugins' inputs and drawing their output | 3.9 ms |

The encoding is not what costs: decoding is a few tenths of a millisecond, and making it cleverer could not save more than that. What costs is one `stat` per input, about 25 µs each on this Windows machine.

Two things were done about that:

- An input that another already covers is not stored. A file that was both measured and read was being checked twice.
- A unit with 64 or more inputs is checked on up to eight threads. With 1000 lore files this took a cached run from 34 ms to 14.7 ms.

One thing was measured and deliberately not done. On Windows, listing a directory returns each entry's size and modification time without a `stat` per file, which would cut the cost several times over. But those figures come from the directory's own records, which lag behind for a file that is still open for writing and are not updated for a file changed through another hard link. A check built on them could call a changed file unchanged, which breaks the one promise the cache makes.

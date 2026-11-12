# Scripting language: design

Why the `.yio` language is the way it is, and how it is implemented in `src/script/`. To write a script, read [scripting-api.md](scripting-api.md) instead.

## What it is for

A small Lisp-like language for `.yio/` plugins that show something about a project's files: scan a directory, read some status lines, draw a table. Two constraints drive every decision.

1. **Specialized for file work.** It is not a general-purpose language. There are no floats, loop forms, macros, imports, or `eval`.
2. **Cacheable.** When nothing a script looked at has changed, its result must be shown without running it. One bash process costs seven to ten times a whole `yio` run, so skipping plugin work is where the cache pays off (see [cache.md](cache.md)).

## A script is a value

A script evaluates to a value, and `yio` draws that value after the script finishes. The table built-ins therefore build values and print nothing: `(col (col-def "File" names) (col-def "Size" sizes))` evaluates to a table.

This is what makes replay simple. The cache stores the value, and a clean run draws it without evaluating anything. A language with `print` would have to record and replay a stream of output instead.

Functions are the one kind of value with no stored form, so a script whose result contains one is an error.

## Every built-in records what it looked at

A built-in that looks at the outside world records what it saw: the contents of a file, the names in a directory, what a path is, the value of an environment variable. That list is stored with the script's value. The script is then a pure function of its recorded inputs, which is the property that makes replay correct.

Inputs are recorded narrowly, so that only a change that could alter the result counts. `file-size` records the size, not the contents. `list-dir` records the names, so editing a file inside does not rerun a script that only listed the directory.

**The rule for adding a built-in**: it may look at or change the outside world only through `src/input.rs`, and must record what it saw or left behind with `Interp::record`. A built-in whose result cannot be replayed from recorded inputs does not belong in the language. That is why there is no clock, randomness, network access, or way to run a program. If one is ever needed, it should mark the result as never cached.

Every built-in also needs a row in `scripting-api.md`. A test fails when the two disagree.

## Changing files

Scripts can write, copy, move, and remove. Three choices keep that safe:

- **Changes stay inside the directory of the script's `yio.md`.** A plugin that comes with an embedded directory cannot reach out of it. Reading is allowed anywhere, because a read cannot do damage and is recorded either way.
- **Nothing is removed recursively**, so one wrong path cannot empty a tree.
- **A change is recorded as the state it left behind.** A cached result is then good only while what the script made is still there, which is how a build tool treats its targets. See "Scripts that change files" in `cache.md`.

## Implementation

| File | Does |
| --- | --- |
| `reader.rs` | Source bytes to forms, each tagged with its line and column |
| `eval.rs` | Evaluates forms: keywords, scopes, functions |
| `value.rs` | Values |
| `builtins.rs` | Built-in functions |
| `render.rs` | A value as terminal text |
| `encode.rs` | A value as bytes, for the cache |
| `mod.rs` | `script::run`, the one entry point |

### Reading

Lists nest at most 256 deep. Everything that walks forms afterwards recurses, so a file nested without bound would overflow the stack instead of reporting an error.

### Evaluating

Evaluation recurses on the native stack. It nests at most 2000 deep, counting each nested expression and each function call, so runaway recursion is an error and not a crash.

The main thread's stack (1 MiB on Windows) is too small for a useful limit, so every script runs on its own thread with a 64 MiB stack, of which only the used part is committed. Measured cost of one level:

| Path | Release | Debug |
| --- | --- | --- |
| A function calling itself | 0.65 KiB | 3.6 KiB |
| A function calling itself through `map` | 1.4 KiB | 5.7 KiB |

At the limit that is at most 2.7 MiB in a release build and 11.2 MiB in a debug build. A test drives each path to the limit, so frames growing past the stack shows up as a failing test run, not as a crash in use.

Values are reference-counted without locking, so they cannot leave the script's thread. `script::run` therefore takes a function that receives the outcome on that thread and reduces it to something plain, such as drawn text or encoded bytes.

Not guarded: a value nested hundreds of thousands deep, built without recursion by wrapping a variable in a new list on every pass of a `map`. Freeing it recurses once per level.

### Values

Lists and strings are shared and never changed in place, so passing one around copies nothing.

Every value except a function has a binary encoding of one tag byte plus payload, which is how it is kept in the cache.

## Decisions

| Question | Decision |
| --- | --- |
| Bytes literal syntax | `#<length>:<bytes>` |
| Is reassignment (`set`) allowed? | Yes |
| Is a script's output its final value, drawn by `yio`? | Yes |
| May scripts read or write outside the directory of their `yio.md`? | Read anywhere, change only inside |
| Does "compressed AST" mean a compact encoding or real compression? | A compact encoding |
| Does a dirty cache refresh itself, or only on `--cache`? | It refreshes itself |
| With a cache, do `.bash` plugins still run on every access? | Yes |
| Does `post/` run after `yio.md` is loaded, or after the chosen command finishes? | After it is loaded |

## Not done

- Removing a directory and its contents in one call.
- Telling a plugin which command was named, or its arguments.
- Text functions that ignore case.

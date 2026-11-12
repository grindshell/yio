# Scripting API

The reference for writing `.yio` scripts: the syntax, the keywords, and every built-in function.

Where scripts live and when they run is in [plugins.md](plugins.md). Why the language is shaped this way is in [scripting-language.md](scripting-language.md).

The tables of keywords and built-ins below are also what `yio` draws when it is run in this directory. See [yio.md](yio.md).

## A script at a glance

```lisp
-- List the notes in lore/ with the status line of each.
(var lore-files (list-dir "lore"))

(if (is-empty lore-files)
  return)

(fn read-status (file)
  (var output ())
  (map (lines (read-file file))
    (fn (line)
      (if (starts-with line "status:")
        (set output (trim (replace line "status:" ""))))))
  output)

(col
  (col-def "File path" lore-files)
  (col-def "File size" (map lore-files file-size))
  (col-def "Status" (map lore-files read-status)))
```

```
File path  File size  Status
---------  ---------  ------
lore/a.md  19         draft
lore/b.md  15
```

A script evaluates to the value of its last top-level expression, or to the value given to a top-level `return`. `yio` draws that value once the script has finished. Nothing is printed along the way, so there is no `print`: a script that wants to show something evaluates to it.

In the call forms below, `[x]` is an argument that may be left out and `x...` is one that may repeat.

## Syntax

| Form | Written as | Notes |
| --- | --- | --- |
| List | `(a b c)` | A call, unless the first item is a keyword. Lists nest at most 256 deep. |
| Symbol | `list-dir`, `<=` | ASCII letters, digits, and `-_?!*/%<>=+`. |
| Integer | `42`, `-7` | 64-bit signed. A token that starts like a number must be one: `12ab` is an error. |
| Boolean | `true`, `false` | |
| String | `"lore"` | UTF-8. Escapes: `\n`, `\t`, `\r`, `\"`, `\\`. May span lines. |
| Bytes | `#3:abc` | A decimal length, a colon, then exactly that many raw bytes of any value. |
| Comment | `-- text` | From `--` at the start of a token to the end of the line. `a--b` is a symbol. |

Whitespace separates tokens and is otherwise ignored. Outside strings, bytes, and comments, the file must be ASCII.

## Values

| Kind | Made by | Notes |
| --- | --- | --- |
| Empty value | `()` | The empty list. There is no separate "nothing". |
| Boolean | `true`, `false` | |
| Integer | `42` | 64-bit signed. There are no floats. |
| String | `"text"` | |
| Bytes | `#3:abc` | |
| List | `list`, and most built-ins | |
| Function | `fn`, or a built-in's name | Can be passed and stored, but cannot be a script's result. |
| Column | `col-def` | |
| Table | `col` | |

- **Truthiness**: only `false` and `()` are false. `0` and `""` are true.
- **Nothing changes in place.** `append`, `replace`, and the rest give a new value, so a value handed to a function cannot be altered behind the caller's back.

## Keywords

Keywords are not functions: they decide which of their parts are evaluated. They cannot be used as variable names.

| Form | Meaning |
| --- | --- |
| `(var name value)` | Declares a variable in the current scope |
| `(set name value)` | Changes the nearest variable with that name |
| `(if cond then [else])` | Evaluates only the branch taken |
| `(do expr...)` | Evaluates each expression in a scope of its own and gives the last |
| `(fn [name] (param...) body...)` | Makes a function, bound to the name when one is given |
| `(return [value])` | Leaves the innermost function, or the script when outside one |
| `(and value...)` | Stops at the first false value and gives it, else gives the last |
| `(or value...)` | Stops at the first true value and gives it, else gives the last |

- `var`, `set`, a named `fn`, an empty body, and an `if` with no `else` whose condition is false all evaluate to `()`.
- Declaring the same name twice in one scope is an error, as is `set` on a name that was never declared.
- `return` may be written bare, without parentheses.
- **Scope** is lexical. The script, each function body, and each `do` has a scope. A function sees, and can change, the variables where it was defined.
- **Functions** check the number of arguments. A named function can call itself. The value of a function is its last expression.
- A script may declare a variable with a built-in's name, but cannot `set` a built-in.
- There are no loop forms, macros, imports, or `eval`. Iteration is `map`, `filter`, and recursion.

## Built-in functions

A call evaluates the function, then the arguments left to right. Every built-in checks the number and kind of its arguments.

A built-in's name is a value like any other, so it can be passed on: `(map files file-size)`.

### Reading files

Each of these records what it looked at. That record is how `yio` knows when a cached result is out of date (see [cache.md](cache.md)).

| Call | Gives | Records |
| --- | --- | --- |
| `(list-dir [path])` | The entries of a directory, sorted by name | The listing |
| `(read-file path)` | A file's contents, which must be UTF-8 | The contents |
| `(file-size path)` | A file's size in bytes | The size |
| `(is-file path)` | Whether the path is a file | What the path is |
| `(is-dir path)` | Whether the path is a directory | What the path is |

- Relative paths resolve against the directory of the script's `yio.md`. Absolute paths and `..` are allowed: a script may read anywhere.
- `list-dir` spells each entry ready to pass on: `(list-dir "lore")` gives `"lore/a.md"`. With no argument it lists the script's own directory, as bare names.
- A missing file or directory is an error for `list-dir`, `read-file`, and `file-size`. For `is-file` and `is-dir` it is simply neither.
- A listing changes when a name is added, removed, or renamed, not when a file inside is edited.

### Environment

| Call | Gives | Records |
| --- | --- | --- |
| `(env name)` | An environment variable's value, or `()` if it is unset | The value |

In a plugin this is also how the built-in flags are read: `(env "YIO_CACHE")` is `"true"` or `"false"`.

### Changing files

| Call | Does | Records |
| --- | --- | --- |
| `(write-file path contents)` | Makes a file hold a string or bytes and gives the path | The file with those contents |
| `(make-dir path)` | Makes a directory, and any missing above it, and gives the path | That the path is a directory |
| `(copy from to)` | Copies a file and gives the new path | The source's contents and the copy's |
| `(move from to)` | Moves a file or directory and gives the new path | That the old path is gone, and what the new one is |
| `(remove path)` | Removes a file or an empty directory and gives `()` | That the path is gone |

- **Where.** A script may change only what is inside the directory of its `yio.md`, and not that directory itself. An absolute path, or one that climbs out with `..`, is an error before anything is touched. `copy` may take its source from anywhere; both ends of a `move` must be inside. The rule looks at how the path is written, so a link inside the directory that points elsewhere is followed.
- **Nothing is removed recursively.** `remove` on a directory that has something in it is an error. Removing a path that is already gone is fine.
- **Unchanged files are left alone.** `write-file` does not rewrite a file that already holds exactly the contents asked for, so its modification time does not move.
- `write-file` creates the directories above the file.
- **With the cache**, a script whose result is still good is not run, so its changes are not made again. They do not need to be: what it made is still there. If something it made is deleted or edited, the script runs again and puts it back. See "Scripts that change files" in [cache.md](cache.md).

### Paths

These work on the text of a path. Nothing is looked up.

| Call | Gives |
| --- | --- |
| `(path-join part...)` | The parts joined with `/` |
| `(base-name path)` | The last component: `a.md` for `lore/a.md` |
| `(extension path)` | What follows the last dot of the base name, without the dot |

- In `path-join`, a part that starts with `/` replaces what came before it, and empty parts are skipped.
- `extension` is empty for `Makefile` and for `.gitignore`.

### Text

| Call | Gives |
| --- | --- |
| `(split text separator)` | A list of the pieces between separators |
| `(join list separator)` | The strings of a list, run together with the separator between |
| `(lines text)` | A list of the lines, without their line endings |
| `(trim text)` | The text without whitespace at either end |
| `(starts-with text prefix)` | Whether the text begins with the prefix |
| `(ends-with text suffix)` | Whether the text ends with the suffix |
| `(contains text part)` | Whether the text has the part in it, or a list has the item |
| `(replace text from to)` | The text with every occurrence replaced |
| `(str value...)` | The values as text, run together |

- `split` and `replace` need a separator or search text that is not empty.
- `contains` also takes a list: `(contains (list 1 2) 2)`.
- `str` uses strings as they are, adds nothing for `()`, and writes anything else the way a script would.
- Matching is exact. Nothing here ignores case.

### Lists

| Call | Gives |
| --- | --- |
| `(list item...)` | A list of the items |
| `(is-empty value)` | Whether a list or string has nothing in it |
| `(len value)` | The items in a list, characters in a string, or bytes |
| `(first list)` | The first item, or `()` for an empty list |
| `(rest list)` | A list of everything after the first item |
| `(nth list index)` | The item at an index, counting from zero |
| `(append list item...)` | A new list with the items added at the end |
| `(map list function)` | A list of the results of calling the function on each item |
| `(filter list function)` | A list of the items for which the function gives a true value |

- `nth` with an index outside the list is an error. `first` of an empty list is not.
- `map` and `filter` take the list first and work only on lists. The function takes one argument.

### Tables

| Call | Gives |
| --- | --- |
| `(col-def header cells)` | A column: a header string and a list of cells |
| `(col column...)` | A table of one or more columns, for `yio` to draw |

### Numbers and comparison

| Call | Gives |
| --- | --- |
| `(+ a b...)` | The sum |
| `(- a b...)` | The first number less the others, or the number negated if alone |
| `(* a b...)` | The product |
| `(/ a b)` | The quotient, truncated toward zero |
| `(% a b)` | The remainder of that division |
| `(= a b)` | Whether two values of any kind are the same |
| `(< a b)` | Whether a comes before b |
| `(> a b)` | Whether a comes after b |
| `(<= a b)` | Whether a comes before b or equals it |
| `(>= a b)` | Whether a comes after b or equals it |
| `(not value)` | True for `false` and `()`, false for anything else |

- Overflow and division by zero are errors, not wraparound.
- `=` compares structurally, lists included. Values of different kinds are simply unequal.
- The ordering operators take two integers or two strings.

## How a value is drawn

| A script that evaluates to | Shows |
| --- | --- |
| The empty value | Nothing |
| A string | The text, on its own line |
| A table | A row of headers, a rule, then the rows, in columns padded to the widest entry |
| A column | A table of that one column |
| A list | Each item in turn, so a script can give a heading, a table, and a note |
| An integer, boolean, or bytes | The value as a script would write it |
| A function | An error |

In a table cell, `()` is blank and line breaks become spaces. Columns of different lengths are padded with blanks. Widths count characters, so wide characters such as CJK can misalign.

## Errors and limits

An error stops the script and reads `line:column: message`, pointing at the call that failed. Columns count characters.

| Limit | Value | Past it |
| --- | --- | --- |
| Nesting of lists in the source | 256 | An error |
| Nesting of evaluation, counting each nested expression and each function call | 2000 | An error |
| Nesting of a script's result | 256 | The result is not cached, so the script runs every time |

The evaluation limit allows several hundred recursive calls, enough to recurse over a list of 500 items.

## What is not here

The clock, randomness, the network, and running other programs. A result that depended on one could not be replayed from the cache. A plugin that needs them is written in bash.

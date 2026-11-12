# The `.yio` scripting language

A small Lisp-like language for plugins (see [plugins.md](plugins.md)). It is made for one job: read files, and evaluate to something `yio` can draw. It is not general purpose, and that is what lets a result be cached and replayed.

Contents: [The shape of a script](#the-shape-of-a-script) · [Syntax and values](#syntax-and-values) · [Keywords](#keywords) · [Built-ins](#built-ins) · [How a value is drawn](#how-a-value-is-drawn) · [Idioms](#idioms) · [Mistakes to avoid](#mistakes-to-avoid)

## The shape of a script

```lisp
-- comments start with two dashes
(var files (list-dir "lore"))

(if (is-empty files)
  return)

(fn status-of (file)
  (var found ())
  (map (lines (read-file file))
    (fn (line)
      (if (starts-with line "status:")
        (set found (trim (replace line "status:" ""))))))
  found)

(col
  (col-def "File" files)
  (col-def "Size" (map files file-size))
  (col-def "Status" (map files status-of)))
```

```
File       Size  Status
---------  ----  ------
lore/a.md  19    draft
lore/b.md  15
```

A script evaluates to the value of its last top-level expression, or to what a top-level `return` is given. `yio` draws that value after the script finishes. There is no `print`: to show something, evaluate to it. To show several things, evaluate to a list of them.

## Syntax and values

| Form | Written as | Notes |
| --- | --- | --- |
| Call | `(name arg arg)` | A list whose first item is a function or keyword |
| Symbol | `list-dir`, `<=` | ASCII letters, digits, and `-_?!*/%<>=+` |
| Integer | `42`, `-7` | 64-bit. There are no floats. |
| Boolean | `true`, `false` | |
| String | `"text"` | UTF-8. Escapes `\n \t \r \" \\`. May span lines. |
| Empty value | `()` | The empty list, and the only "nothing" |
| Comment | `-- text` | To the end of the line |

- Only `false` and `()` are false. `0` and `""` are true.
- Nothing changes in place: `append`, `replace`, and the rest give a new value.
- Outside strings and comments the file must be ASCII.

## Keywords

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

- `return` may be written bare, without parentheses.
- Scope is lexical. A function sees, and can `set`, the variables where it was defined. That is how a `map` accumulates into an outer variable. A `do` is the same: a `var` inside it ends with it, and a `set` inside it reaches the variables outside.
- A function can be defined inside another, and `return` inside one leaves only that function.
- Declaring a name twice in one scope is an error, as is `set` on an undeclared name.
- Functions check their argument count. A named function can call itself.
- There are no loops, macros, or imports. Iteration is `map`, `filter`, and recursion (a few hundred levels deep at most).

## Built-ins

`[x]` may be left out and `x...` may repeat. Every built-in checks the number and kind of its arguments. A built-in's name is a value, so it can be passed: `(map files file-size)`.

### Reading files

| Call | Gives |
| --- | --- |
| `(list-dir [path])` | The entries of a directory, sorted by name |
| `(read-file path)` | A file's contents, which must be UTF-8 |
| `(file-size path)` | A file's size in bytes |
| `(is-file path)` | Whether the path is a file |
| `(is-dir path)` | Whether the path is a directory |

- Relative paths resolve against the directory of the script's `yio.md`, not against `.yio/pre/`. A script may read anywhere.
- `list-dir` gives entries ready to pass on: `(list-dir "lore")` gives `"lore/a.md"`. With no argument it lists the script's directory as bare names. It lists files and directories alike, and does not descend.
- A missing path is an error for `list-dir`, `read-file`, and `file-size`. Check with `is-dir` or `is-file` first when it may be absent.

### The run

| Call | Gives |
| --- | --- |
| `(env name)` | An environment variable's value, or `()` if it is unset |
| `(args)` | A list of the command-line words that are for the script's `yio.md` |

`(env "YIO_HELP")` and the other built-in flags are `"true"` or `"false"`: strings, so compare with `(= (env "YIO_HELP") "true")`.

### Changing files

| Call | Does |
| --- | --- |
| `(write-file path contents)` | Makes a file hold a string or bytes and gives the path |
| `(make-dir path)` | Makes a directory, and any missing above it, and gives the path |
| `(copy from to)` | Copies a file and gives the new path |
| `(move from to)` | Moves a file or directory and gives the new path |
| `(remove path)` | Removes a file or an empty directory and gives `()` |

- A script may change only what is inside the directory of its `yio.md`. An absolute path or one that climbs out with `..` is an error. `copy` may take its source from anywhere.
- `remove` does not remove a directory that has something in it.
- `write-file` creates missing directories, and leaves a file alone that already holds those contents.

### Paths

| Call | Gives |
| --- | --- |
| `(path-join part...)` | The parts joined with `/` |
| `(base-name path)` | The last component: `a.md` for `lore/a.md` |
| `(extension path)` | What follows the last dot of the base name, without the dot |

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

- Matching is exact and case-sensitive. There are no regular expressions.
- `join` takes strings only, so turn numbers into text with `str` first.
- `lines` drops `\r\n` endings as well as `\n`.
- **Text cannot be turned into a number**, and a string's characters cannot be reached one at a time. A number read out of a file stays a string: it can be shown and compared, not added. The integers a script can do arithmetic on come from `len`, `file-size`, and its own literals. When a plugin has to sum numbers written in files, say so to the user and use a bash plugin for that part.

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

`map` and `filter` take the list first, then a function of one argument. `nth` outside the list is an error; `first` of an empty list is `()`.

### Tables

| Call | Gives |
| --- | --- |
| `(col-def header cells)` | A column: a header string and a list of cells |
| `(col column...)` | A table of one or more columns |

A table is built by column, not by row. Columns of different lengths are padded with blanks, and a `()` cell is blank.

### Layout and style

| Call | Gives |
| --- | --- |
| `(style value name...)` | The value with its text in the named styles |
| `(box value [title])` | The value with a border around it |
| `(row value...)` | The values side by side |
| `(rule)` | A line as wide as what it sits among |
| `(bar part whole [width])` | A bar filled to the share that part is of whole, and the percentage |
| `(tree label children)` | The label with a list of children drawn under it as branches |
| `(fields labels values)` | A list of labels, each with its value beside it |

- Styles: `bold`, `dim`, `italic`, `underline`, `red`, `green`, `yellow`, `blue`, `magenta`, `cyan`, `white`, `gray`. They show only on a terminal; layout is the same either way. `style` takes any value, a number or a whole table included.
- These nest freely: a table in a box, boxes in a row, a bar or a styled string in a table cell.
- `bar` needs a whole above 0. The width defaults to 20.
- A child of a `tree` may be another `tree`.

```lisp
(row
  (box
    (fields
      (list "Notes" "Finished" "Progress")
      (list 4 2 (bar 2 4 12)))
    "Lore")
  (box
    (tree "lore" (list
      "dragons.md"
      (tree "old" (list "first.md"))
      (style "rivers.md" "yellow")))
    "Files"))
```

```
┌─ Lore ──────────────────────┐  ┌─ Files ────────┐
│ Notes     4                 │  │ lore           │
│ Finished  2                 │  │ ├─ dragons.md  │
│ Progress  ██████░░░░░░  50% │  │ ├─ old         │
└─────────────────────────────┘  │ │  └─ first.md │
                                 │ └─ rivers.md   │
                                 └────────────────┘
```

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

The ordering operators take two integers or two strings. Overflow and division by zero are errors.

## How a value is drawn

| A script that evaluates to | Shows |
| --- | --- |
| The empty value | Nothing |
| A string | The text, on its own line |
| A table or a column | Headers, a rule, then the rows |
| A box, row, tree, and so on | That |
| A list | Each item in turn, one under the other |
| An integer or boolean | The value |
| A function | An error |

So a heading, a table, and a note is `(list "Heading" (col ...) "Note")`, and a `()` in that list (such as an `if` that was not taken) adds nothing.

## Idioms

**Stop early when there is nothing to do.** A plugin runs for every command, so most begin with a guard and a bare `return`, which shows nothing.

```lisp
(if (not (is-dir "notes"))
  return)
```

**Accumulate with `set`** from inside a `map`, since there are no loops:

```lisp
(var total 0)
(map files (fn (file) (set total (+ total (file-size file)))))
```

**Rows to columns.** Collect each row as a list, then pull a column out of the rows:

```lisp
(var rows (map files (fn (file) (list file (file-size file)))))
(fn column (header index)
  (col-def header (map rows (fn (row) (nth row index)))))
(col (column "File" 0) (column "Bytes" 1))
```

**A value from a `name: value` line.** This looks at every line of the file and keeps the last match, and what it gives is a string even when it reads `words: 1800`:

```lisp
(fn field (file name)
  (var found "")
  (map (lines (read-file file))
    (fn (line)
      (if (starts-with line (str name ":"))
        (set found (trim (replace line (str name ":") ""))))))
  found)
```

**Only some files:** `(filter (list-dir "notes") (fn (path) (= (extension path) "md")))`.

**Walk a directory tree** by recursion, with `is-dir` deciding where to descend:

```lisp
(fn branch (path)
  (if (is-dir path)
    (tree (base-name path) (map (list-dir path) branch))
    (base-name path)))
(branch "src")
```

## Mistakes to avoid

- **Expecting output along the way.** Only the script's final value is shown.
- **Paths relative to the plugin file.** They are relative to the directory of the `yio.md`, two levels above `.yio/pre/`.
- **`(map function list)`.** The list comes first.
- **Treating `"false"` as false.** `(env "YIO_HELP")` is a non-empty string, which is true. Compare it with `"true"`.
- **Building a table by row.** `col` takes columns.
- **Reaching for the clock, randomness, the network, or another program.** None exist here, on purpose: a result that depended on one could not be replayed from the cache. Write that plugin in bash.
- **Sorting, regular expressions, case-insensitive matching, turning text into a number.** Not available. `list-dir` is already sorted by name.
- **Returning a function**, or a list with one in it, is an error.

# Docs

> Tasks for the documentation in `docs/`

These work here as `yio search` and `yio links`, and from the directory above as `yio docs search` and `yio docs links`, because the `yio.md` there embeds this one.

## search [...TERMS]

> Show the scripting API, or only the calls that mention every word

This command does not draw the table. A plugin, `.yio/pre/0000_api.yio`, does, before the command runs. A plugin is told the command line, so it sees `search` and the words after it, and it reads the rows out of `scripting-api.md`, so the table is never behind the document.

A row is kept when the call, its group, or its description mentions every word. Matching is exact, so `File` and `file` differ.

With a cache (`yio --cache`), repeating a search replays the table without running the plugin, until `scripting-api.md` or the plugin changes.

```bash
# Nothing left to do: the plugin has already drawn the table.
```

## links

> Check that the links in these docs lead to files that exist

Only links to files are followed, not ones to websites. A link inside a code block or code span is an example, so it is skipped.

```bash
status=0
for file in *.md; do
  # Drop code blocks, then code spans, then keep each link's file.
  targets="$(awk '/^ *```/ { code = !code; next } !code' "$file" \
    | sed 's/`[^`]*`//g' \
    | grep -o ']([^)#]*' \
    | sed 's/^](//' \
    | sort -u)"
  for target in $targets; do
    case "$target" in
      *://*) continue ;;
    esac
    if [ ! -e "$target" ]; then
      echo "$file: $target does not exist"
      status=1
    fi
  done
done

if [ "$status" -eq 0 ]; then
  echo "Every link leads somewhere."
fi
exit "$status"
```

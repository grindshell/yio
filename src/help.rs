//! Renders the description of a command, shown for `--help` and for a
//! command that has no script to run.

use crate::command::{Builtin, Command, Flag, FlagType};

/// Describes `command`: its help text, usage line, subcommands, and flags.
/// `path` holds the command names leading to it from the root.
pub fn render(path: &[&str], command: &Command, verbose: bool) -> String {
    let mut out = String::new();
    if let Some(help) = &command.short_help {
        out.push_str(help);
        out.push_str("\n\n");
    }
    if verbose && let Some(help) = &command.long_help {
        out.push_str(help);
        out.push_str("\n\n");
    }

    let mut prefix = String::from("yio");
    for name in path {
        prefix.push(' ');
        prefix.push_str(name);
    }
    // A subcommand is only recognized before the first argument, so running
    // the command and naming one of its subcommands are separate forms.
    let mut forms = Vec::new();
    if !command.scripts.is_empty() || command.subcommands.is_empty() {
        forms.push(with_signature(&prefix, command));
    }
    if !command.subcommands.is_empty() {
        forms.push(format!("{prefix} <command>"));
    }
    for (i, form) in forms.iter().enumerate() {
        out.push_str(if i == 0 { "Usage: " } else { "       " });
        out.push_str(form);
        out.push_str(" [flags]\n");
    }

    if !command.subcommands.is_empty() {
        out.push_str("\nCommands:\n");
        let rows: Vec<Row> = command.subcommands.iter().map(command_row).collect();
        table(&mut out, &rows, verbose);
    }

    out.push_str("\nFlags:\n");
    let mut rows: Vec<Row> = command.flags.iter().map(flag_row).collect();
    let builtins = Builtin::ALL.into_iter();
    rows.extend(builtins.filter_map(|builtin| builtin_row(command, builtin)));
    table(&mut out, &rows, false);
    out
}

struct Row<'a> {
    label: String,
    text: String,
    /// Extra lines shown under the row in verbose listings.
    detail: Option<&'a str>,
}

fn table(out: &mut String, rows: &[Row], verbose: bool) {
    let width = rows.iter().map(|row| row.label.chars().count()).max();
    let width = width.unwrap_or(0);
    for row in rows {
        let line = format!("  {:width$}  {}", row.label, row.text);
        out.push_str(line.trim_end());
        out.push('\n');
        if verbose && let Some(detail) = row.detail {
            for line in detail.lines() {
                if !line.is_empty() {
                    out.push_str("      ");
                    out.push_str(line);
                }
                out.push('\n');
            }
        }
    }
}

fn command_row(command: &Command) -> Row<'_> {
    Row {
        label: with_signature(&command.name, command),
        text: command.short_help.clone().unwrap_or_default(),
        detail: command.long_help.as_deref(),
    }
}

fn flag_row(flag: &Flag) -> Row<'_> {
    let mut label = flag_label(flag.short, flag.long.as_deref());
    if flag.ty == FlagType::String {
        label.push_str(&format!(" <{}>", flag.name));
    }
    let mut text = flag.description.clone().unwrap_or_default();
    if let Some(default) = &flag.default {
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(&format!("[default: {default}]"));
    }
    Row { label, text, detail: None }
}

/// The row for a built-in, showing only the spellings that still reach it.
/// The command's own flags shadow built-ins spelling by spelling, exactly as
/// the parser resolves them.
fn builtin_row<'a>(command: &Command, builtin: Builtin) -> Option<Row<'a>> {
    let short = builtin.short().filter(|c| command.short_flag(*c).is_none());
    let long = Some(builtin.long()).filter(|name| command.long_flag(name).is_none());
    if short.is_none() && long.is_none() {
        return None;
    }
    Some(Row {
        label: flag_label(short, long),
        text: builtin.description().to_string(),
        detail: None,
    })
}

fn flag_label(short: Option<char>, long: Option<&str>) -> String {
    match (short, long) {
        (Some(short), Some(long)) => format!("-{short}, --{long}"),
        (Some(short), None) => format!("-{short}"),
        (None, Some(long)) => format!("    --{long}"),
        (None, None) => String::new(),
    }
}

/// `name` followed by the command's arguments as its heading writes them.
fn with_signature(name: &str, command: &Command) -> String {
    match command.signature() {
        signature if signature.is_empty() => name.to_string(),
        signature => format!("{name} {signature}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown;

    const SOURCE: &str = "\
> Project tasks

## build (TARGET) [PROFILE]

> Build a target

Longer description.

**FLAGS**
- JOBS
  - flags: -j --jobs
  - default: 4
  - description: Parallel jobs

```
echo
```

### clean

## test
";

    #[test]
    fn lists_top_level_commands() {
        let root = markdown::parse(SOURCE).unwrap();
        assert_eq!(
            render(&[], &root, false),
            "\
Project tasks

Usage: yio <command> [flags]

Commands:
  build (TARGET) [PROFILE]  Build a target
  test

Flags:
  -h, --help          Show help for a command
  -V, --verbose       Show verbose help text
  -v, --version       Print the yio version
      --cache         Write the interpreted yio.md to .yio/cache.yio
      --remove-cache  Remove .yio/cache.yio
"
        );
    }

    #[test]
    fn verbose_listing_adds_paragraph_text() {
        let root = markdown::parse(SOURCE).unwrap();
        let listing = render(&[], &root, true);
        assert!(listing.contains("  build (TARGET) [PROFILE]  Build a target\n      Longer description.\n"));
    }

    #[test]
    fn lists_a_command_with_its_own_flags() {
        let root = markdown::parse(SOURCE).unwrap();
        let listing = render(&["build"], &root.subcommands[0], false);
        assert!(listing.contains(
            "Usage: yio build (TARGET) [PROFILE] [flags]\n       yio build <command> [flags]\n"
        ));
        assert!(listing.contains("  -j, --jobs <JOBS>   Parallel jobs [default: 4]\n"));
    }

    #[test]
    fn usage_omits_the_form_that_cannot_be_used() {
        let root = markdown::parse("## group (X)\n### leaf (Y)\n```\necho\n```\n").unwrap();
        let group = &root.subcommands[0];
        let listing = render(&["group"], group, false);
        assert!(listing.contains("Usage: yio group <command> [flags]\n\n"));
        let listing = render(&["group", "leaf"], &group.subcommands[0], false);
        assert!(listing.contains("Usage: yio group leaf (Y) [flags]\n\n"));
    }

    #[test]
    fn shows_the_argument_that_takes_the_rest() {
        let root = markdown::parse("## search (WHERE) [...TERMS]\n```\necho\n```\n## all [...X]\n").unwrap();
        let listing = render(&[], &root, false);
        assert!(listing.contains("  search (WHERE) [...TERMS]\n  all [...X]\n"));
        let listing = render(&["search"], &root.subcommands[0], false);
        assert!(listing.contains("Usage: yio search (WHERE) [...TERMS] [flags]\n\n"));
    }

    #[test]
    fn shadowed_builtin_spellings_are_not_advertised() {
        let source = "\
## a

**FLAGS**
- HOT
  - flags: -h --hot
  - type: boolean
- V
  - flags: -V --verbose
  - type: boolean
";
        let root = markdown::parse(source).unwrap();
        let listing = render(&["a"], &root.subcommands[0], false);
        assert!(listing.contains("  -h, --hot\n"));
        assert!(listing.contains("      --help          Show help for a command\n"));
        assert!(!listing.contains("Show verbose help text"));
        assert!(listing.contains("  -v, --version       Print the yio version\n"));
    }
}

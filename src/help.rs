//! Renders the description of a command, shown for `--help` and for a
//! command that has no script to run.

use crate::command::{Builtin, Command, Flag, FlagType, Param};

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

    out.push_str("Usage: yio");
    for name in path {
        out.push(' ');
        out.push_str(name);
    }
    for param in &command.params {
        out.push(' ');
        out.push_str(&param_label(param));
    }
    if !command.subcommands.is_empty() {
        out.push_str(" <command>");
    }
    out.push_str(" [flags]\n");

    if !command.subcommands.is_empty() {
        out.push_str("\nCommands:\n");
        let rows: Vec<Row> = command.subcommands.iter().map(command_row).collect();
        table(&mut out, &rows, verbose);
    }

    out.push_str("\nFlags:\n");
    let mut rows: Vec<Row> = command.flags.iter().map(flag_row).collect();
    let overridden = |b: &Builtin| command.flags.iter().any(|f| f.name == b.env_name());
    rows.extend(
        Builtin::ALL
            .into_iter()
            .filter(|builtin| !overridden(builtin))
            .map(builtin_row),
    );
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
    let mut label = command.name.clone();
    for param in &command.params {
        label.push(' ');
        label.push_str(&param_label(param));
    }
    Row {
        label,
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

fn builtin_row<'a>(builtin: Builtin) -> Row<'a> {
    Row {
        label: flag_label(builtin.short(), Some(builtin.long())),
        text: builtin.description().to_string(),
        detail: None,
    }
}

fn flag_label(short: Option<char>, long: Option<&str>) -> String {
    match (short, long) {
        (Some(short), Some(long)) => format!("-{short}, --{long}"),
        (Some(short), None) => format!("-{short}"),
        (None, Some(long)) => format!("    --{long}"),
        (None, None) => String::new(),
    }
}

fn param_label(param: &Param) -> String {
    if param.required {
        format!("({})", param.name)
    } else {
        format!("[{}]", param.name)
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
        assert!(listing.contains("Usage: yio build (TARGET) [PROFILE] <command> [flags]\n"));
        assert!(listing.contains("  -j, --jobs <JOBS>   Parallel jobs [default: 4]\n"));
    }
}

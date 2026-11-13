//! Looks for boolean variables that a script uses in a way that cannot
//! work. See "Boolean checks" in `docs/cli-parsing.md`.
//!
//! A boolean reaches a script as the string `true` or `false`. It is never
//! empty and never a number, so `[ -n "$RELEASE" ]`, `[ "$RELEASE" = 1 ]`,
//! and `(( RELEASE ))` all give the same answer whichever it is.
//!
//! This is a heuristic over the text of a script, one line at a time, and
//! not a bash parser. It speaks up only about a use it recognizes and is
//! sure is wrong. Anything else passes without comment, so no warning does
//! not mean a script is right.

use crate::command::{Builtin, Command, FlagType};

/// Something in a command's script that cannot do what it looks like.
#[derive(Debug, PartialEq)]
pub struct Warning {
    /// The line of the `yio.md`, counting from 1.
    pub line: usize,
    pub message: String,
}

/// Checks how the scripts of `command` use its boolean flags and the
/// built-in ones.
pub fn booleans(command: &Command) -> Vec<Warning> {
    let names = boolean_names(command);
    let mut warnings = Vec::new();
    for (i, script) in command.scripts.iter().enumerate() {
        let first = command.lines.get(i).copied().unwrap_or(1);
        for (offset, line) in script.lines().enumerate() {
            let mut messages = Vec::new();
            check_text(line, &names, &mut messages);
            for message in messages {
                let warning = Warning { line: first + offset, message };
                if !warnings.contains(&warning) {
                    warnings.push(warning);
                }
            }
        }
    }
    warnings
}

/// The variables that hold `true` or `false` in the command's scripts.
fn boolean_names(command: &Command) -> Vec<&str> {
    let builtins = [Builtin::Help, Builtin::Verbose, Builtin::Cache, Builtin::RemoveCache, Builtin::DryRun];
    // A command may reuse a built-in's name for something of its own.
    let own = |name: &str| {
        command.params.iter().any(|param| param.name == name)
            || command.rest.as_deref() == Some(name)
            || command.flags.iter().any(|flag| flag.name == name)
    };
    let mut names: Vec<&str> =
        builtins.into_iter().map(Builtin::env_name).filter(|name| !own(name)).collect();
    let flags = command.flags.iter().filter(|flag| flag.ty == FlagType::Boolean);
    names.extend(flags.map(|flag| flag.name.as_str()));
    names
}

/// Checks one line, or the inside of a command substitution on it.
fn check_text(text: &str, names: &[&str], messages: &mut Vec<String>) {
    // Nearly every line mentions no boolean at all, and a comment is not code.
    if text.trim_start().starts_with('#') || !names.iter().any(|name| text.contains(name)) {
        return;
    }
    let line = lex(text);
    alternates(text, names, messages);
    tests(&line.tokens, names, messages);
    for inner in &line.arithmetic {
        arithmetic(inner, names, messages);
    }
    for inner in &line.substitutions {
        check_text(inner, names, messages);
    }
}

fn empty_test(name: &str) -> String {
    format!(
        "`{name}` is always `true` or `false`, never empty, so this test gives the same \
         answer either way; compare it with `true` instead: [ \"${name}\" = true ]"
    )
}

/// `${NAME:+text}` and `${NAME+text}`: the text is used whenever the
/// variable is set, and a boolean always is.
fn alternates(text: &str, names: &[&str], messages: &mut Vec<String>) {
    for (start, _) in text.match_indices("${") {
        let rest = &text[start + 2..];
        let end = rest.find(|c: char| !is_name_char(c)).unwrap_or(rest.len());
        let (name, rest) = rest.split_at(end);
        let operator = ["+", ":+"].into_iter().find(|operator| rest.starts_with(operator));
        if let Some(operator) = operator
            && names.contains(&name)
        {
            messages.push(format!(
                "`{name}` is always set, so `${{{name}{operator}...}}` is used even when it is \
                 `false`"
            ));
        }
    }
}

/// `(( NAME ))` and `$(( NAME ))`: arithmetic takes `true` and `false` for
/// the names of variables, which are unset, so both count as 0.
fn arithmetic(inner: &str, names: &[&str], messages: &mut Vec<String>) {
    for word in inner.split(|c: char| !is_name_char(c)) {
        if names.contains(&word) {
            messages.push(format!(
                "`{word}` is `true` or `false`, and arithmetic reads either one as 0; compare \
                 it with `true` instead: [ \"${word}\" = true ]"
            ));
        }
    }
}

/// `[ ... ]`, `[[ ... ]]`, and `test ...`.
fn tests(tokens: &[Token], names: &[&str], messages: &mut Vec<String>) {
    let mut i = 0;
    while i < tokens.len() {
        let closing = match &tokens[i] {
            Token::Word(word) if !word.quoted => match word.text.as_str() {
                "[" => Some("]"),
                "[[" => Some("]]"),
                "test" if starts_command(tokens, i) => None,
                _ => {
                    i += 1;
                    continue;
                }
            },
            _ => {
                i += 1;
                continue;
            }
        };
        i += 1;
        // The words of one condition, up to `&&`, `||`, `-a`, `-o`, or a
        // bracket, each of which starts another.
        let mut condition: Vec<&Word> = Vec::new();
        while i < tokens.len() {
            let word = match &tokens[i] {
                // `test` has no closing word: it ends with its command.
                Token::Break if closing.is_none() => break,
                Token::Break => None,
                Token::Word(word) if !word.quoted && Some(word.text.as_str()) == closing => break,
                Token::Word(word) if !word.quoted && matches!(word.text.as_str(), "-a" | "-o") => {
                    None
                }
                Token::Word(word) => Some(word),
            };
            match word {
                Some(word) => condition.push(word),
                None => {
                    check_condition(&condition, names, messages);
                    condition.clear();
                }
            }
            i += 1;
        }
        check_condition(&condition, names, messages);
    }
}

/// Whether the word at `i` is where a command name goes.
fn starts_command(tokens: &[Token], i: usize) -> bool {
    match i.checked_sub(1).map(|before| &tokens[before]) {
        None | Some(Token::Break) => true,
        Some(Token::Word(word)) => {
            let leads = ["if", "elif", "while", "until", "then", "do", "else", "!", "{"];
            !word.quoted && leads.contains(&word.text.as_str())
        }
    }
}

/// One condition of a test: `-n X`, `X`, `X = Y`, `X -eq Y`.
fn check_condition(condition: &[&Word], names: &[&str], messages: &mut Vec<String>) {
    // A leading `!` only turns the answer around.
    let negations = condition.iter().take_while(|word| !word.quoted && word.text == "!").count();
    let boolean = |word: &Word| word.boolean(names);
    match &condition[negations..] {
        [only] => {
            if let Some(name) = boolean(only) {
                messages.push(empty_test(name));
            }
        }
        [operator, operand] if !operator.quoted && matches!(operator.text.as_str(), "-n" | "-z") => {
            if let Some(name) = boolean(operand) {
                messages.push(empty_test(name));
            }
        }
        [left, operator, right] if !operator.quoted => {
            let (name, other) = match (boolean(left), boolean(right)) {
                // Two booleans compared with each other is fine.
                (Some(_), Some(_)) | (None, None) => return,
                (Some(name), None) => (name, right),
                (None, Some(name)) => (name, left),
            };
            match operator.text.as_str() {
                "=" | "==" | "!=" => {
                    if let Some(value) = other.literal() {
                        match value {
                            "true" | "false" => {}
                            "" => messages.push(empty_test(name)),
                            value => messages.push(format!(
                                "`{name}` is always `true` or `false`, so it never equals `{value}`"
                            )),
                        }
                    }
                }
                "-eq" | "-ne" | "-lt" | "-le" | "-gt" | "-ge" => messages.push(format!(
                    "`{name}` is `true` or `false`, not a number, so `{}` cannot compare it",
                    operator.text
                )),
                _ => {}
            }
        }
        _ => {}
    }
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// One word of a command, with its quotes removed.
#[derive(Debug, Default)]
struct Word {
    text: String,
    /// Whether any of it was in quotes. `"["` is not the start of a test.
    quoted: bool,
    /// Whether any of it was kept from the shell by single quotes or a
    /// backslash, which makes a `$` in it an ordinary character.
    literal: bool,
    /// Whether any of it is worked out when the script runs.
    dynamic: bool,
}

impl Word {
    /// The boolean this word is a reference to and nothing else: `$NAME`,
    /// `${NAME}`, or either in double quotes.
    fn boolean<'a>(&self, names: &[&'a str]) -> Option<&'a str> {
        if self.literal {
            return None;
        }
        let name = self.text.strip_prefix('$')?;
        let name = name.strip_prefix('{').and_then(|name| name.strip_suffix('}')).unwrap_or(name);
        names.iter().copied().find(|known| *known == name)
    }

    /// The text of this word, if it is the same every time the script runs
    /// and is not a pattern.
    fn literal(&self) -> Option<&str> {
        let pattern = !self.quoted && self.text.contains(['*', '?', '[']);
        (!self.dynamic && !pattern).then_some(self.text.as_str())
    }
}

#[derive(Debug)]
enum Token {
    Word(Word),
    /// Something that ends a command or a condition: `;`, `&&`, `|`, `(`.
    Break,
}

/// What is on one line of a script.
#[derive(Debug, Default)]
struct Line {
    tokens: Vec<Token>,
    /// The inside of each `(( ... ))` and `$(( ... ))`.
    arithmetic: Vec<String>,
    /// The inside of each `$( ... )` and `` ` ... ` ``, which is a command
    /// of its own.
    substitutions: Vec<String>,
}

/// Splits a line into words the way the shell would, near enough.
fn lex(text: &str) -> Line {
    let chars: Vec<char> = text.chars().collect();
    let mut line = Line::default();
    let mut word: Option<Word> = None;
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' => {
                line.tokens.extend(word.take().map(Token::Word));
                i += 1;
            }
            // A comment runs to the end of the line.
            '#' if word.is_none() => break,
            '\'' => {
                let word = word.get_or_insert_default();
                word.quoted = true;
                word.literal = true;
                let end = find(&chars, i + 1, '\'');
                word.text.extend(&chars[i + 1..end]);
                i = end + 1;
            }
            '"' => {
                let word = word.get_or_insert_default();
                word.quoted = true;
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    match chars[i] {
                        '\\' if i + 1 < chars.len() => {
                            // Only these lose their backslash in double quotes.
                            if matches!(chars[i + 1], '$' | '`' | '"' | '\\') {
                                word.literal = true;
                            } else {
                                word.text.push('\\');
                            }
                            word.text.push(chars[i + 1]);
                            i += 2;
                        }
                        '$' | '`' => i = expansion(&chars, i, word, &mut line),
                        other => {
                            word.text.push(other);
                            i += 1;
                        }
                    }
                }
                i += 1;
            }
            '\\' => {
                let word = word.get_or_insert_default();
                word.literal = true;
                word.text.extend(chars.get(i + 1));
                i += 2;
            }
            '$' | '`' => i = expansion(&chars, i, word.get_or_insert_default(), &mut line),
            '(' if word.is_none() && chars.get(i + 1) == Some(&'(') => {
                let end = closing(&chars, i + 2, '(', ')');
                line.arithmetic.push(chars[i + 2..end].iter().collect());
                line.tokens.push(Token::Break);
                // Past the second `)` as well.
                i = end + 2;
            }
            ';' | '|' | '&' | '(' | ')' | '<' | '>' => {
                line.tokens.extend(word.take().map(Token::Word));
                if !matches!(line.tokens.last(), Some(Token::Break)) {
                    line.tokens.push(Token::Break);
                }
                i += 1;
            }
            other => {
                word.get_or_insert_default().text.push(other);
                i += 1;
            }
        }
    }
    line.tokens.extend(word.map(Token::Word));
    line
}

/// Reads what starts with the `$` or backtick at `start` into `word`, and
/// returns where it ends.
fn expansion(chars: &[char], start: usize, word: &mut Word, line: &mut Line) -> usize {
    let next = chars.get(start + 1).copied();
    let after = chars.get(start + 2).copied();
    let inside = |from: usize, end: usize| -> String { chars[from..end].iter().collect() };
    match (chars[start], next, after) {
        ('`', ..) => {
            let end = find(chars, start + 1, '`');
            line.substitutions.push(inside(start + 1, end));
            word.dynamic = true;
            end + 1
        }
        ('$', Some('('), Some('(')) => {
            let end = closing(chars, start + 3, '(', ')');
            line.arithmetic.push(inside(start + 3, end));
            word.dynamic = true;
            end + 2
        }
        ('$', Some('('), _) => {
            let end = closing(chars, start + 2, '(', ')');
            line.substitutions.push(inside(start + 2, end));
            word.dynamic = true;
            end + 1
        }
        ('$', Some('{'), _) => {
            let end = closing(chars, start + 2, '{', '}');
            word.text.push_str("${");
            word.text.push_str(&inside(start + 2, end));
            word.text.push('}');
            word.dynamic = true;
            end + 1
        }
        ('$', Some(c), _) if is_name_char(c) || "@*#?!$-".contains(c) => {
            word.text.push('$');
            word.dynamic = true;
            start + 1
        }
        // A `$` with nothing to expand is just a dollar sign.
        _ => {
            word.text.push('$');
            start + 1
        }
    }
}

/// The index of the next `wanted` at or after `from`, or the end.
fn find(chars: &[char], from: usize, wanted: char) -> usize {
    (from..chars.len()).find(|&i| chars[i] == wanted).unwrap_or(chars.len())
}

/// The index of the `close` that matches an `open` just before `from`, or
/// the end.
fn closing(chars: &[char], from: usize, open: char, close: char) -> usize {
    let mut depth = 0_usize;
    for (i, c) in chars.iter().enumerate().skip(from) {
        if *c == open {
            depth += 1;
        } else if *c == close {
            if depth == 0 {
                return i;
            }
            depth -= 1;
        }
    }
    chars.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown;

    /// The messages for one line of a script whose command has a boolean
    /// flag `RELEASE`, a string flag `JOBS`, and an argument `TARGET`.
    fn check(line: &str) -> Vec<String> {
        let source = format!(
            "## build (TARGET)\n\n**FLAGS**\n- RELEASE\n  - flags: -r\n  - type: boolean\n- JOBS\n  - flags: -j\n\n```\n{line}\n```\n"
        );
        let root = markdown::parse(&source).unwrap();
        booleans(&root.subcommands[0]).into_iter().map(|warning| warning.message).collect()
    }

    const EMPTY: &str = "`RELEASE` is always `true` or `false`, never empty, so this test gives \
                         the same answer either way; compare it with `true` instead: \
                         [ \"$RELEASE\" = true ]";

    #[test]
    fn uses_that_work_pass() {
        for line in [
            r#"if [ "$RELEASE" = true ]; then echo yes; fi"#,
            r#"if [ "$RELEASE" = "true" ]; then echo yes; fi"#,
            r#"[ $RELEASE == true ] && echo yes"#,
            r#"[[ $RELEASE == true ]] && echo yes"#,
            r#"[[ "${RELEASE}" != false ]] && echo yes"#,
            r#"[ true = "$RELEASE" ] || exit"#,
            r#"test "$RELEASE" = false && exit"#,
            // Running it: `true` and `false` are commands.
            r#"if $RELEASE; then echo yes; fi"#,
            r#""$RELEASE" && echo yes"#,
            r#"while ! $RELEASE; do break; done"#,
            r#"case "$RELEASE" in true) echo yes;; esac"#,
            // Handing it on.
            r#"cargo build --release="$RELEASE""#,
            r#"echo "release is $RELEASE""#,
            r#"other=$RELEASE"#,
            // Compared with something only known when the script runs.
            r#"[ "$RELEASE" = "$WANTED" ]"#,
            r#"[ "$RELEASE" = "$(cat wanted)" ]"#,
            r#"[ "$RELEASE" = "$YIO_VERBOSE" ]"#,
            // A pattern can match `true`.
            r#"[[ $RELEASE == t* ]]"#,
            // Part of a longer word is not the variable alone.
            r#"[ "x$RELEASE" = xtrue ]"#,
            r#"[ -n "$RELEASE$TARGET" ]"#,
            r#"[ -f "out/$RELEASE" ]"#,
            // A default is never used, but it does no harm.
            r#"release="${RELEASE:-false}""#,
            // Other variables are not booleans.
            r#"[ -n "$TARGET" ] && [ "$JOBS" -gt 1 ] && (( JOBS > 2 ))"#,
            r#"[ -n "$RELEASED" ] && echo "${RELEASED:+x}""#,
            // Not code.
            r#"# [ -n "$RELEASE" ] is always true"#,
            r#"echo '[ -n "$RELEASE" ]' "\$RELEASE""#,
            r#"[ -n '$RELEASE' ] && [ -n "\$RELEASE" ] && [ -n \$RELEASE ]"#,
            r#"echo test "$RELEASE""#,
        ] {
            assert_eq!(check(line), [""; 0], "{line}");
        }
    }

    #[test]
    fn testing_for_empty_is_flagged() {
        for line in [
            r#"if [ -n "$RELEASE" ]; then echo yes; fi"#,
            r#"if [ -z "$RELEASE" ]; then echo no; fi"#,
            r#"[ "$RELEASE" ] && echo yes"#,
            r#"[ $RELEASE ] && echo yes"#,
            r#"[[ $RELEASE ]] && echo yes"#,
            r#"[[ -n ${RELEASE} ]] && echo yes"#,
            r#"[ ! -z "$RELEASE" ] && echo yes"#,
            r#"if ! [ ! "$RELEASE" ]; then echo yes; fi"#,
            r#"test -n "$RELEASE" && echo yes"#,
            r#"if test "$RELEASE"; then echo yes; fi"#,
            r#"[ "$RELEASE" != "" ] && echo yes"#,
            r#"[ "" = "$RELEASE" ] && echo no"#,
            // One condition among several.
            r#"[ -f out ] && [ -n "$RELEASE" ]"#,
            r#"[ -f out -a -n "$RELEASE" ]"#,
            r#"[[ -f out && -n $RELEASE ]]"#,
            r#"[[ -f out || ( -n $RELEASE ) ]]"#,
            // Inside a command substitution.
            r#"mode=$( [ -n "$RELEASE" ] && echo release )"#,
            r#"mode="$(if [ -n "$RELEASE" ]; then echo release; fi)""#,
            r#"mode=`[ -n "$RELEASE" ] && echo release`"#,
        ] {
            assert_eq!(check(line), [EMPTY], "{line}");
        }
    }

    #[test]
    fn comparing_with_anything_but_true_and_false_is_flagged() {
        let never = |value: &str| {
            vec![format!("`RELEASE` is always `true` or `false`, so it never equals `{value}`")]
        };
        assert_eq!(check(r#"[ "$RELEASE" = 1 ] && echo yes"#), never("1"));
        assert_eq!(check(r#"[ "$RELEASE" == "yes" ] && echo yes"#), never("yes"));
        assert_eq!(check(r#"[[ $RELEASE != 0 ]] && echo yes"#), never("0"));
        assert_eq!(check(r#"[ True = "$RELEASE" ] && echo yes"#), never("True"));
        assert_eq!(check(r#"test "$RELEASE" = on && echo yes"#), never("on"));
        // A quoted pattern is only text.
        assert_eq!(check(r#"[[ $RELEASE == "t*" ]]"#), never("t*"));
    }

    #[test]
    fn treating_it_as_a_number_is_flagged() {
        let number = |operator: &str| {
            vec![format!(
                "`RELEASE` is `true` or `false`, not a number, so `{operator}` cannot compare it"
            )]
        };
        assert_eq!(check(r#"[ "$RELEASE" -eq 1 ] && echo yes"#), number("-eq"));
        assert_eq!(check(r#"[[ 0 -lt $RELEASE ]] && echo yes"#), number("-lt"));

        let arithmetic = vec![
            "`RELEASE` is `true` or `false`, and arithmetic reads either one as 0; compare it \
             with `true` instead: [ \"$RELEASE\" = true ]"
                .to_string(),
        ];
        assert_eq!(check("if (( RELEASE )); then echo yes; fi"), arithmetic);
        assert_eq!(check("(( $RELEASE == 1 )) && echo yes"), arithmetic);
        assert_eq!(check("count=$(( JOBS + RELEASE ))"), arithmetic);
        assert_eq!(check(r#"echo "jobs: $(( JOBS * (1 + ${RELEASE}) ))""#), arithmetic);
    }

    #[test]
    fn an_alternate_value_is_flagged() {
        let alternate = |operator: &str| {
            vec![format!(
                "`RELEASE` is always set, so `${{RELEASE{operator}...}}` is used even when it is \
                 `false`"
            )]
        };
        assert_eq!(check(r#"cargo build ${RELEASE:+--release}"#), alternate(":+"));
        assert_eq!(check(r#"cargo build "${RELEASE+--release}""#), alternate("+"));
    }

    #[test]
    fn the_built_in_flags_are_booleans_too() {
        assert_eq!(check(r#"[ -n "$YIO_VERBOSE" ] && set -x"#).len(), 1);
        assert_eq!(check(r#"[ "$YIO_CACHE" = 1 ] && exit"#).len(), 1);
        // Except the version, which is a string.
        assert_eq!(check(r#"[ -n "$YIO_VERSION" ] && echo yes"#), [""; 0]);

        // And except one the command has taken for something else.
        let source = "## a (YIO_HELP)\n\n**FLAGS**\n- YIO_CACHE\n  - flags: -c\n- YIO_VERBOSE\n  - flags: -l\n  - type: boolean\n\n```\n[ -n \"$YIO_HELP\" ] && [ -n \"$YIO_CACHE\" ]\n[ -n \"$YIO_VERBOSE\" ]\n```\n";
        let root = markdown::parse(source).unwrap();
        let lines: Vec<usize> =
            booleans(&root.subcommands[0]).into_iter().map(|warning| warning.line).collect();
        assert_eq!(lines, [12]);
    }

    #[test]
    fn warnings_name_the_line_in_the_file() {
        let source = "\
# Tasks

## build

**FLAGS**
- RELEASE
  - flags: -r
  - type: boolean

```bash
set -e
if [ -n \"$RELEASE\" ]; then
  echo release
fi
```

Between.

```bash
[ \"$RELEASE\" = 1 ] && [ \"$RELEASE\" = 1 ]
[ -n \"$RELEASE\" ] && (( RELEASE ))
```
";
        let root = markdown::parse(source).unwrap();
        let found = booleans(&root.subcommands[0]);
        let lines: Vec<usize> = found.iter().map(|warning| warning.line).collect();
        // The same finding twice on one line is reported once.
        assert_eq!(lines, [12, 20, 21, 21]);
        assert!(found[1].message.ends_with("never equals `1`"));
    }

    #[test]
    fn odd_lines_do_not_break_it() {
        for line in [
            "",
            "$",
            "\"$RELEASE",
            "'$RELEASE",
            "[ -n \"$RELEASE",
            "$(( RELEASE",
            "$( [ -n",
            "${RELEASE",
            "`$RELEASE",
            "[ $RELEASE \\",
            "(( ))",
            "echo $RELEASE \\",
            "[[ ]] [ ] test",
        ] {
            let _ = check(line);
        }
    }
}

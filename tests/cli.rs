//! Runs the built `yio` the way it is used: in a directory, with a command
//! line. The unit tests cover each part on its own. These cover what only
//! shows once the parts are together, such as what a cached run tells a
//! plugin, and what bash makes of a script.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// A directory under the system's temporary directory that is removed when
/// the test ends. Names must be unique across tests, which run in parallel.
struct Scratch(PathBuf);

/// What one run of `yio` printed and exited with.
#[derive(Debug)]
struct Ran {
    stdout: String,
    stderr: String,
    code: i32,
}

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("yio-cli-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    fn write(&self, name: &str, text: &str) {
        let path = self.0.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    /// Runs `yio` in this directory with the words of `line` as its
    /// arguments.
    fn yio(&self, line: &str) -> Ran {
        self.yio_with(&line.split_whitespace().collect::<Vec<_>>())
    }

    /// Runs `yio` in this directory with exactly these arguments.
    fn yio_with(&self, args: &[&str]) -> Ran {
        let output = Command::new(env!("CARGO_BIN_EXE_yio"))
            .args(args)
            .current_dir(&self.0)
            // Output is not a terminal here, so nothing is styled unless
            // the environment of the test run forces it.
            .env_remove("CLICOLOR_FORCE")
            .output()
            .unwrap();
        Ran {
            stdout: String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n"),
            stderr: String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n"),
            code: output.status.code().unwrap_or(-1),
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Whether there is a bash that works, which the tests that run a script
/// need. Where there is none, such as a Windows machine whose only bash is
/// a WSL launcher with nothing installed behind it, there is nothing for
/// them to learn, and they say so and pass.
fn bash_works() -> bool {
    static WORKS: OnceLock<bool> = OnceLock::new();
    *WORKS.get_or_init(|| {
        let scratch = Scratch::new("probe");
        scratch.write("yio.md", "## probe\n\n```\nexit 7\n```\n");
        let probe = scratch.yio("probe");
        if probe.code != 7 {
            eprintln!("no usable bash, so the tests that run scripts are skipped ({probe:?})");
        }
        probe.code == 7
    })
}

#[test]
fn every_line_of_a_script_runs() {
    if !bash_works() {
        return;
    }
    let scratch = Scratch::new("lines");
    scratch.write(
        "yio.md",
        "## lines\n\n```\ntrue\nfalse\n```\n\n## words\n\n```\necho one\necho two\n```\n\n## code\n\n```\nexit 7\n```\n\n```\necho not reached\n```\n",
    );

    // No line of this script has a space in it. On Windows such a script
    // used to reach Git Bash unquoted, which ran its first line only.
    assert_eq!(scratch.yio("lines").code, 1);
    let words = scratch.yio("words");
    assert_eq!((words.stdout.as_str(), words.code), ("one\ntwo\n", 0));
    // The first block that fails stops the rest, and its code is yio's.
    let code = scratch.yio("code");
    assert_eq!((code.stdout.as_str(), code.code), ("", 7));
}

#[test]
fn values_reach_a_script_exactly_as_typed() {
    if !bash_works() {
        return;
    }
    let scratch = Scratch::new("values");
    scratch.write(
        "yio.md",
        "## show (TARGET) [...REST]\n\n**FLAGS**\n- NOTE\n  - flags: --note\n- LOUD\n  - flags: --loud\n  - type: boolean\n\n\
         ```\nprintf '<%s>\\n' \"$TARGET\" \"$NOTE\" \"$LOUD\" \"$YIO_VERSION\" \"$YIO_DRY_RUN\" \"${#REST[@]}\" \"${REST[@]}\"\n```\n",
    );
    // Everything a shell would expand, split, or take for a quote.
    let ran = scratch.yio_with(&["show", "it's $HOME *", "--note", "a \"b\" \\ c", "--", "-x", "two words", "", "`id`"]);
    let version = env!("CARGO_PKG_VERSION");
    let expected = format!(
        "<it's $HOME *>\n<a \"b\" \\ c>\n<false>\n<{version}>\n<false>\n<4>\n<-x>\n<two words>\n<>\n<`id`>\n"
    );
    assert_eq!((ran.stdout.as_str(), ran.code), (expected.as_str(), 0), "{ran:?}");
}

#[test]
fn a_script_runs_in_the_directory_of_its_yio_md() {
    if !bash_works() {
        return;
    }
    let scratch = Scratch::new("where");
    scratch.write("yio.md", "## here\n\n```\nbasename \"$PWD\"\n```\n\n## docs\n\n[docs](docs/yio.md)\n");
    scratch.write("docs/yio.md", "## there\n\n```\nbasename \"$PWD\"\n```\n");
    let top = scratch.0.file_name().unwrap().to_string_lossy().into_owned();
    assert_eq!(scratch.yio("here").stdout, format!("{top}\n"));
    assert_eq!(scratch.yio("docs there").stdout, "docs\n");
}

#[test]
fn a_bash_plugin_is_told_the_flags_and_the_words() {
    if !bash_works() {
        return;
    }
    let scratch = Scratch::new("bash-plugin");
    scratch.write("yio.md", "## one [...WORDS]\n\n```\n# Nothing to do.\n```\n");
    scratch.write(".yio/pre/1_told.bash", "printf '%s|' \"$YIO_VERBOSE\" \"$YIO_DRY_RUN\" \"$#\" \"$@\"\necho\n");
    scratch.write(".yio/post/1_fails.bash", "[ \"$1\" != fail ] || exit 3\n");

    let ran = scratch.yio_with(&["-V", "one", "two words", "*", "it's", "--dry-run"]);
    assert_eq!(ran.stdout.lines().next(), Some("true|true|4|one|two words|*|it's|"), "{ran:?}");
    assert_eq!(ran.code, 0, "{ran:?}");
    // Each plugin is announced on stderr when asked to be verbose.
    assert!(ran.stderr.contains("yio: pre plugin told (.yio/pre/1_told.bash)\n"), "{ran:?}");

    // A plugin that fails stops the run and is named.
    let ran = scratch.yio("fail");
    assert_eq!(ran.stderr, "yio: plugin `.yio/post/1_fails.bash` failed with exit code 3\n");
    assert_eq!(ran.code, 2);
}

#[test]
fn a_dry_run_shows_the_command_and_runs_nothing() {
    let scratch = Scratch::new("dry");
    scratch.write(
        "yio.md",
        "## make (NAME)\n\n> Makes a file\n\n**FLAGS**\n- MODE\n  - flags: --mode\n  - default: 644\n\n```\ntouch \"$NAME\"\nchmod \"$MODE\" \"$NAME\"\n```\n",
    );
    let ran = scratch.yio("make marker --dry-run");
    let expected = "# Dry run of `yio make`: nothing below has been run.\nNAME='marker'\nMODE='644'\n\n\
                    # yio.md, line 11\ntouch \"$NAME\"\nchmod \"$MODE\" \"$NAME\"\n";
    assert_eq!((ran.stdout.as_str(), ran.stderr.as_str(), ran.code), (expected, "", 0));
    assert!(!scratch.0.join("marker").exists());

    // What a real run would refuse, a dry run refuses.
    let ran = scratch.yio("make --dry-run");
    assert_eq!((ran.stderr.as_str(), ran.code), ("yio: missing required argument `(NAME)` for `yio make`\n", 2));
    // A name that is nearly right is pointed out.
    let ran = scratch.yio("mak marker");
    assert_eq!((ran.stderr.as_str(), ran.code), ("yio: unknown command `mak` for `yio`; did you mean `make`?\n", 2));
    let ran = scratch.yio("make marker --mod 600");
    assert_eq!(ran.stderr, "yio: unknown flag `--mod` for `yio make`; did you mean `--mode`?\n");
}

#[test]
fn a_directory_without_a_yio_md_still_describes_the_cli() {
    let scratch = Scratch::new("empty");
    for line in ["", "build", "--cache"] {
        let ran = scratch.yio(line);
        assert_eq!(
            (ran.stdout.as_str(), ran.stderr.as_str(), ran.code),
            ("", "yio: no yio.md in the current directory\n", 2),
            "`yio {line}`"
        );
    }
    let help = scratch.yio("--help");
    assert!(help.stdout.starts_with("Y I Oughta: a task runner for bash."), "{help:?}");
    assert!(help.stdout.contains("\nUsage: yio [flags]\n"), "{help:?}");
    assert_eq!(help.code, 0);
    assert_eq!(scratch.yio("--version").stdout, format!("yio {}\n", env!("CARGO_PKG_VERSION")));
    // Nothing was left behind.
    assert_eq!(fs::read_dir(&scratch.0).unwrap().count(), 0);
}

#[test]
fn a_reader_that_stops_reading_is_not_a_failure() {
    let scratch = Scratch::new("closed");
    scratch.write("yio.md", "## one\n\n> First\n\n```\n# Nothing to do.\n```\n");
    scratch.write(".yio/pre/1_table.yio", r#"(col (col-def "Name" (list "a" "b")))"#);
    // As `yio | head` does once `head` has what it wants: nothing is
    // reading standard output by the time `yio` writes to it.
    for line in ["", "--help", "one", "one --dry-run", "--version"] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_yio"))
            .args(line.split_whitespace())
            .current_dir(&scratch.0)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        drop(child.stdout.take());
        let output = child.wait_with_output().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!((stderr.as_ref(), output.status.code()), ("", Some(0)), "`yio {line}`");
    }
}

#[cfg(unix)]
#[test]
fn the_last_script_takes_over_the_process() {
    use std::os::unix::process::ExitStatusExt;

    let scratch = Scratch::new("exec");
    scratch.write(
        "yio.md",
        "## pids\n\n```\necho \"first $$\"\n```\n\n```\necho \"last $$\"\n```\n\n```\n# Nothing to do.\n```\n\n\
         ## stopped\n\n```\nkill -TERM $$\n```\n\n\
         ## stopped-early\n\n```\nkill -TERM $$\n```\n\n```\necho not reached\n```\n",
    );
    let yio = |line: &str| {
        let child = Command::new(env!("CARGO_BIN_EXE_yio"))
            .arg(line)
            .current_dir(&scratch.0)
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let pid = child.id();
        let output = child.wait_with_output().unwrap();
        (pid, String::from_utf8_lossy(&output.stdout).into_owned(), output.status)
    };

    // The last block with anything to do runs as the process `yio` was.
    // The ones before it are children.
    let (pid, stdout, status) = yio("pids");
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 2, "{stdout}");
    assert_ne!(lines[0], format!("first {pid}"));
    assert_eq!(lines[1], format!("last {pid}"));
    assert!(status.success());

    // So a signal that ends it is what the caller sees, as from bash.
    assert_eq!(yio("stopped").2.signal(), Some(15));
    // An earlier block is reported as a shell would, and stops the rest.
    let (_, stdout, status) = yio("stopped-early");
    assert_eq!((stdout.as_str(), status.code()), ("", Some(143)));
}

#[test]
fn a_cache_does_not_change_what_a_plugin_is_told() {
    let scratch = Scratch::new("told");
    // `status/yio.md` brings help text and no commands, so the command tree
    // has nothing that says where it is embedded.
    scratch.write(
        "yio.md",
        "## status\n\n[status](status/yio.md)\n\n## docs\n\n[docs](docs/yio.md)\n\n## other\n\n```\n# Nothing to do.\n```\n",
    );
    scratch.write("status/yio.md", "> Status of the project\n");
    scratch.write("status/.yio/pre/1_told.yio", r#"(str "status: [" (join (args) " ") "]")"#);
    scratch.write("docs/yio.md", "## search [...TERMS]\n\n```\n# Answered by a plugin.\n```\n");
    scratch.write("docs/.yio/pre/1_told.yio", r#"(str "docs: [" (join (args) " ") "]")"#);

    let cases = [
        ("other", "status: []\ndocs: []\n"),
        ("docs search a b", "status: []\ndocs: [search a b]\n"),
        ("status", "status: []\ndocs: []\n"),
    ];
    let check = |when: &str| {
        for (line, told) in cases {
            let ran = scratch.yio(line);
            assert!(ran.stdout.starts_with(told), "{when}, `yio {line}`: {ran:?}");
            assert_eq!(ran.code, 0, "{when}, `yio {line}`: {ran:?}");
        }
    };
    check("without a cache");
    assert_eq!(scratch.yio("--cache").code, 0);
    // Twice, so that results stored by the first cached run are used too.
    check("with a cache");
    check("with a cache, again");
}

#[test]
fn the_cache_follows_the_files_it_was_made_from() {
    let scratch = Scratch::new("follows");
    scratch.write("yio.md", "## one\n\n> First\n");
    assert_eq!(scratch.yio("--cache").stdout, "wrote .yio/cache.yio\n");
    assert!(scratch.yio("").stdout.contains("  one  First\n"));

    scratch.write("yio.md", "## one\n\n> First\n\n## two\n\n> Second, and longer\n");
    let listing = scratch.yio("").stdout;
    assert!(listing.contains("  two  Second, and longer\n"), "{listing}");

    assert_eq!(scratch.yio("--remove-cache").stdout, "removed .yio/cache.yio\n");
    assert!(!scratch.0.join(".yio").exists());
}

#[test]
fn the_cache_flags_do_not_need_what_the_command_requires() {
    let scratch = Scratch::new("required");
    scratch.write(
        "yio.md",
        "## deploy (TARGET)\n\n**FLAGS**\n- YES\n  - flags: --yes\n  - type: boolean\n  - required: true\n\n```\n# Nothing to do.\n```\n",
    );
    let ran = scratch.yio("deploy");
    assert_eq!((ran.stderr.as_str(), ran.code), ("yio: missing required argument `(TARGET)` for `yio deploy`\n", 2));

    let ran = scratch.yio("deploy --cache");
    assert_eq!((ran.stdout.as_str(), ran.code), ("wrote .yio/cache.yio\n", 0), "{ran:?}");
    let ran = scratch.yio("deploy --remove-cache");
    assert_eq!((ran.stdout.as_str(), ran.code), ("removed .yio/cache.yio\n", 0), "{ran:?}");
}

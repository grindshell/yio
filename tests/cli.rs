//! Runs the built `yio` the way it is used: in a directory, with a command
//! line. The unit tests cover each part on its own. These cover what only
//! shows once the parts are together, such as what a cached run tells a
//! plugin, and what bash makes of a script.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

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

    /// Runs `yio` in this directory with `line` as its arguments.
    fn yio(&self, line: &str) -> Ran {
        let output = Command::new(env!("CARGO_BIN_EXE_yio"))
            .args(line.split_whitespace())
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

#[test]
fn every_line_of_a_script_runs() {
    let scratch = Scratch::new("lines");
    scratch.write(
        "yio.md",
        "## probe\n\n```\nexit 7\n```\n\n## lines\n\n```\ntrue\nfalse\n```\n\n## words\n\n```\necho one\necho two\n```\n",
    );
    // The rest needs a bash that works. Where there is none, such as a
    // Windows machine whose only bash is a WSL launcher with nothing
    // installed behind it, there is nothing to learn.
    let probe = scratch.yio("probe");
    if probe.code != 7 {
        eprintln!("skipped: no usable bash ({probe:?})");
        return;
    }

    // No line of this script has a space in it. On Windows such a script
    // used to reach Git Bash unquoted, which ran its first line only.
    assert_eq!(scratch.yio("lines").code, 1);
    let words = scratch.yio("words");
    assert_eq!((words.stdout.as_str(), words.code), ("one\ntwo\n", 0));
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

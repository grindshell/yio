//! Plugins: scripts in `.yio/pre/` and `.yio/post/` that run around the
//! loading of the `yio.md` beside them. See "Plugins" in
//! `docs/scripting-language.md`.

use std::cell::RefCell;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::cli::Builtins;
use crate::input::Vars;
use crate::run::{self, Script};
use crate::script::{self, render};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Stage {
    /// Before the `yio.md` is read.
    Pre,
    /// After the whole command tree is ready.
    Post,
}

impl Stage {
    fn name(self) -> &'static str {
        match self {
            Stage::Pre => "pre",
            Stage::Post => "post",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    /// `.bash`: run with bash.
    Bash,
    /// `.yio`: evaluated, and its value rendered.
    Yio,
}

/// One plugin file, named `NUMBER_NAME.bash` or `NUMBER_NAME.yio`.
#[derive(Debug, PartialEq)]
pub struct Plugin {
    /// Plugins run in order of this number.
    pub number: u64,
    /// For messages.
    pub name: String,
    pub file: String,
    pub kind: Kind,
}

impl Plugin {
    fn parse(file: &str) -> Option<Plugin> {
        let (stem, kind) = match (file.strip_suffix(".bash"), file.strip_suffix(".yio")) {
            (Some(stem), _) => (stem, Kind::Bash),
            (_, Some(stem)) => (stem, Kind::Yio),
            _ => return None,
        };
        let (number, name) = stem.split_once('_')?;
        if name.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        let number = number.parse().ok()?;
        Some(Plugin { number, name: name.to_string(), file: file.to_string(), kind })
    }
}

/// A path as written in messages, with forward slashes on every platform.
fn show(path: &Path) -> String {
    path.display().to_string().replace('\\', "/")
}

/// Finds the plugins of one stage for the `yio.md` in `dir`, in the order
/// they run: by number, then by file name.
///
/// Every file in the directory must be a plugin, so that a misnamed one is
/// reported instead of silently never running. Names starting with a dot
/// are left alone.
pub fn discover(dir: &Path, stage: Stage) -> Result<Vec<Plugin>, String> {
    let folder = dir.join(".yio").join(stage.name());
    let entries = match fs::read_dir(&folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("could not list `{}`: {error}", show(&folder))),
    };
    let mut plugins = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("could not list `{}`: {error}", show(&folder)))?;
        let file = entry.file_name().to_string_lossy().into_owned();
        if file.starts_with('.') {
            continue;
        }
        let is_file = entry.file_type().is_ok_and(|kind| !kind.is_dir());
        match Plugin::parse(&file).filter(|_| is_file) {
            Some(plugin) => plugins.push(plugin),
            None => {
                return Err(format!(
                    "`{}` is not a plugin; plugins are files named NUMBER_NAME.bash or NUMBER_NAME.yio",
                    show(&folder.join(&file))
                ));
            }
        }
    }
    plugins.sort_by(|a, b| (a.number, &a.file).cmp(&(b.number, &b.file)));
    Ok(plugins)
}

/// Runs one plugin of the `yio.md` in `dir` and gives the text it wants
/// shown. A bash plugin writes to the terminal itself, so it gives nothing.
pub fn run(dir: &Path, stage: Stage, plugin: &Plugin, vars: Vars) -> Result<String, String> {
    let relative = format!(".yio/{}/{}", stage.name(), plugin.file);
    let shown = show(&dir.join(&relative));
    match plugin.kind {
        Kind::Bash => {
            let dir = (!dir.as_os_str().is_empty()).then_some(dir);
            match run::run_one(Script::File(&relative), vars, dir)? {
                0 => Ok(String::new()),
                code => Err(format!("plugin `{shown}` failed with exit code {code}")),
            }
        }
        Kind::Yio => {
            let source = fs::read(dir.join(&relative))
                .map_err(|error| format!("could not read `{shown}`: {error}"))?;
            match script::run(&source, dir, vars, |outcome| render::render(&outcome.value)) {
                Ok(Ok(text)) => Ok(text),
                Ok(Err(message)) => Err(format!("{shown}: {message}")),
                Err(error) => Err(format!("{shown}:{error}")),
            }
        }
    }
}

/// Runs plugins at the right moments of one `yio` run, each at most once.
pub struct Plugins {
    enabled: bool,
    verbose: bool,
    /// The `YIO_*` variables plugins see.
    vars: Vec<(&'static str, String)>,
    /// Directories whose `pre` plugins have run, in that order.
    visited: RefCell<Vec<PathBuf>>,
}

impl Plugins {
    /// `flags` are the built-in flags found on the command line by
    /// `cli::scan`. Plugins do not run for `--version` or `--remove-cache`,
    /// neither of which uses the `yio.md`.
    pub fn new(flags: Builtins) -> Self {
        Plugins {
            enabled: !flags.version && !flags.remove_cache,
            verbose: flags.verbose,
            vars: flags.env(),
            visited: RefCell::default(),
        }
    }

    /// Call before the `yio.md` in `dir` is read, or before a cached copy of
    /// it is trusted. Runs that directory's `pre` plugins the first time.
    pub fn before(&self, dir: &Path) -> Result<(), String> {
        if !self.enabled || self.visited.borrow().iter().any(|seen| seen == dir) {
            return Ok(());
        }
        self.visited.borrow_mut().push(dir.to_path_buf());
        self.stage(dir, Stage::Pre)
    }

    /// Call once the command tree is ready. Runs the `post` plugins of every
    /// directory whose `yio.md` went into it, in the order they were reached.
    pub fn after(&self) -> Result<(), String> {
        let visited = self.visited.borrow().clone();
        visited.iter().try_for_each(|dir| self.stage(dir, Stage::Post))
    }

    fn stage(&self, dir: &Path, stage: Stage) -> Result<(), String> {
        for plugin in discover(dir, stage)? {
            if self.verbose {
                let file = show(&dir.join(".yio").join(stage.name()).join(&plugin.file));
                eprintln!("yio: {} plugin {} ({file})", stage.name(), plugin.name);
            }
            print!("{}", run(dir, stage, &plugin, &self.vars)?);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Scratch;

    fn names(plugins: &[Plugin]) -> Vec<&str> {
        plugins.iter().map(|plugin| plugin.file.as_str()).collect()
    }

    #[test]
    fn parses_plugin_names() {
        let parsed = |file| Plugin::parse(file).map(|p| (p.number, p.name, p.kind));
        assert_eq!(parsed("0000_build_view.yio"), Some((0, "build_view".to_string(), Kind::Yio)));
        assert_eq!(parsed("12_sync.bash"), Some((12, "sync".to_string(), Kind::Bash)));
        assert_eq!(parsed("18446744073709551615_max.yio"), Some((u64::MAX, "max".to_string(), Kind::Yio)));
        for bad in [
            "build_view.yio",
            "_view.yio",
            "1_.yio",
            "1.yio",
            "+1_a.yio",
            "-1_a.yio",
            "1_a.sh",
            "1_a",
            "18446744073709551616_overflow.yio",
        ] {
            assert_eq!(parsed(bad), None, "{bad}");
        }
    }

    #[test]
    fn plugins_run_in_number_order_then_by_name() {
        let scratch = Scratch::new("plugin-order");
        for file in ["10_last.yio", "2_b.bash", "0002_a.yio", "1_first.bash", ".gitkeep"] {
            scratch.write(&format!(".yio/pre/{file}"), "");
        }
        scratch.write(".yio/post/5_after.yio", "");

        let pre = discover(scratch.root(), Stage::Pre).unwrap();
        // Leading zeros do not matter: 0002 and 2 are the same number.
        assert_eq!(names(&pre), ["1_first.bash", "0002_a.yio", "2_b.bash", "10_last.yio"]);
        assert_eq!(names(&discover(scratch.root(), Stage::Post).unwrap()), ["5_after.yio"]);
    }

    #[test]
    fn a_directory_without_plugins_has_none() {
        let scratch = Scratch::new("plugin-none");
        assert_eq!(discover(scratch.root(), Stage::Pre).unwrap(), []);
        scratch.write(".yio/cache.yio", "");
        assert_eq!(discover(scratch.root(), Stage::Post).unwrap(), []);
    }

    #[test]
    fn a_misnamed_file_is_an_error() {
        let scratch = Scratch::new("plugin-misnamed");
        scratch.write(".yio/pre/1_ok.yio", "");
        scratch.write(".yio/pre/notes.txt", "");
        let error = discover(scratch.root(), Stage::Pre).unwrap_err();
        assert!(error.ends_with(
            "/.yio/pre/notes.txt` is not a plugin; plugins are files named NUMBER_NAME.bash or NUMBER_NAME.yio"
        ), "{error}");

        // A directory with a plugin's name is not one either.
        let scratch = Scratch::new("plugin-directory");
        scratch.write(".yio/pre/1_dir.yio/inner", "");
        assert!(discover(scratch.root(), Stage::Pre).unwrap_err().contains("is not a plugin"));
    }

    #[test]
    fn a_yio_plugin_is_evaluated_in_its_directory_and_rendered() {
        let scratch = Scratch::new("plugin-yio");
        scratch.write("lore/a.md", "x");
        scratch.write("lore/b.md", "xyz");
        scratch.write(
            ".yio/pre/0000_view.yio",
            r#"(var files (list-dir "lore"))
(col (col-def "File" files) (col-def "Size" (map files file-size)))"#,
        );
        let plugins = discover(scratch.root(), Stage::Pre).unwrap();
        let text = run(scratch.root(), Stage::Pre, &plugins[0], &[]).unwrap();
        assert_eq!(text, "File       Size\n---------  ----\nlore/a.md  1\nlore/b.md  3\n");
    }

    #[test]
    fn a_yio_plugin_sees_the_variables_of_the_run() {
        let scratch = Scratch::new("plugin-vars");
        scratch.write(
            ".yio/pre/1_cache.yio",
            r#"(if (= (env "YIO_CACHE") "true") return) "not caching""#,
        );
        let plugin = &discover(scratch.root(), Stage::Pre).unwrap()[0];
        let run = |cache: &str| {
            run(scratch.root(), Stage::Pre, plugin, &[("YIO_CACHE", cache.to_string())]).unwrap()
        };
        assert_eq!(run("false"), "not caching\n");
        assert_eq!(run("true"), "");
    }

    #[test]
    fn a_failing_yio_plugin_names_its_file_and_position() {
        let scratch = Scratch::new("plugin-fail");
        scratch.write(".yio/post/3_bad.yio", "(var x 1)\n  (nope x)");
        scratch.write(".yio/post/4_fn.yio", "(fn (x) x)");
        let plugins = discover(scratch.root(), Stage::Post).unwrap();

        let error = run(scratch.root(), Stage::Post, &plugins[0], &[]).unwrap_err();
        assert!(error.ends_with("/.yio/post/3_bad.yio:2:4: `nope` is not defined"), "{error}");
        let error = run(scratch.root(), Stage::Post, &plugins[1], &[]).unwrap_err();
        assert!(
            error.ends_with("/.yio/post/4_fn.yio: a script's result cannot contain a function"),
            "{error}"
        );
    }

    #[test]
    fn pre_plugins_run_once_per_directory_and_not_for_version() {
        let scratch = Scratch::new("plugin-once");
        // A plugin that fails, so that running it is observable.
        scratch.write(".yio/pre/1_boom.yio", "(nope)");

        let plugins = Plugins::new(Builtins::default());
        assert!(plugins.before(scratch.root()).is_err());
        assert!(plugins.before(scratch.root()).is_ok());

        for flags in [
            Builtins { version: true, ..Builtins::default() },
            Builtins { remove_cache: true, ..Builtins::default() },
        ] {
            let plugins = Plugins::new(flags);
            assert!(plugins.before(scratch.root()).is_ok());
            assert!(plugins.after().is_ok());
        }
    }
}

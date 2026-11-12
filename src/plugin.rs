//! Plugins: scripts in `.yio/pre/` and `.yio/post/` that run around the
//! loading of the `yio.md` beside them. See "Plugins" in
//! `docs/scripting-language.md`.

use std::cell::RefCell;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::cache::{Session, Unit, UnitKind};
use crate::cli::Builtins;
use crate::input::{self, Freshness, Input, Vars};
use crate::run::{self, Script};
use crate::script::{self, encode, render};

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

/// What running a plugin produced.
#[derive(Debug)]
pub struct Ran {
    /// The text it wants shown. A bash plugin writes to the terminal
    /// itself, so it has none.
    pub text: String,
    /// For a `.yio` plugin, what its value depended on, starting with the
    /// script file itself.
    pub inputs: Vec<Input>,
    /// For a `.yio` plugin, its value in stored form, when it can be stored.
    pub value: Option<Vec<u8>>,
}

/// Runs one plugin of the `yio.md` in `dir`.
pub fn run(dir: &Path, stage: Stage, plugin: &Plugin, vars: Vars) -> Result<Ran, String> {
    let relative = format!(".yio/{}/{}", stage.name(), plugin.file);
    let shown = show(&dir.join(&relative));
    match plugin.kind {
        Kind::Bash => {
            let dir = (!dir.as_os_str().is_empty()).then_some(dir);
            match run::run_one(Script::File(&relative), vars, dir)? {
                0 => Ok(Ran { text: String::new(), inputs: Vec::new(), value: None }),
                code => Err(format!("plugin `{shown}` failed with exit code {code}")),
            }
        }
        Kind::Yio => {
            let (source, script) = input::read_file(&dir.join(&relative))
                .map_err(|error| format!("could not read `{shown}`: {error}"))?;
            let ran = script::run(&source, dir, vars, |outcome| {
                let text = render::render(&outcome.value);
                (text, encode::encode(&outcome.value).ok(), outcome.inputs)
            });
            match ran {
                Ok((Ok(text), value, mut inputs)) => {
                    inputs.insert(0, script);
                    input::prune(&mut inputs);
                    Ok(Ran { text, inputs, value })
                }
                Ok((Err(message), ..)) => Err(format!("{shown}: {message}")),
                Err(error) => Err(format!("{shown}:{error}")),
            }
        }
    }
}

/// Runs plugins at the right moments of one `yio` run, each at most once.
pub struct Plugins<'a> {
    enabled: bool,
    verbose: bool,
    /// The `YIO_*` variables plugins see.
    vars: Vec<(&'static str, String)>,
    /// Directories whose `pre` plugins have run, in that order.
    visited: RefCell<Vec<PathBuf>>,
    session: &'a Session,
    /// When this run started, which is the as-of time of what it records.
    started: u64,
}

impl<'a> Plugins<'a> {
    /// `flags` are the built-in flags found on the command line by
    /// `cli::scan`. Plugins do not run for `--version` or `--remove-cache`,
    /// neither of which uses the `yio.md`.
    pub fn new(flags: Builtins, session: &'a Session, started: u64) -> Self {
        Plugins {
            enabled: !flags.version && !flags.remove_cache,
            verbose: flags.verbose,
            vars: flags.env(),
            visited: RefCell::default(),
            session,
            started,
        }
    }

    /// Whether plugins run at all in this run.
    pub fn enabled(&self) -> bool {
        self.enabled
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
            let source = show(&dir.join(".yio").join(stage.name()).join(&plugin.file));
            let replayed = self.replay(&source, &plugin);
            if self.verbose {
                let from = if replayed.is_some() { ", from the cache" } else { "" };
                eprintln!("yio: {} plugin {} ({source}){from}", stage.name(), plugin.name);
            }
            let text = match replayed {
                Some(text) => text,
                None => self.execute(dir, stage, &plugin, source)?,
            };
            print!("{text}");
        }
        Ok(())
    }

    /// The text of a `.yio` plugin's stored value, if there is one and
    /// nothing it depended on has changed. The plugin is then not run.
    fn replay(&self, source: &str, plugin: &Plugin) -> Option<String> {
        if plugin.kind != Kind::Yio {
            // A bash plugin can read anything, so it can never be replayed.
            return None;
        }
        let unit = self.session.find(UnitKind::Script, source)?;
        let settled = match input::check(&unit.inputs, unit.as_of, self.started, &self.vars) {
            Freshness::Dirty => return None,
            Freshness::Clean => None,
            Freshness::Settled(inputs) => Some(inputs),
        };
        let text = render::render(&encode::decode(&unit.body).ok()?).ok()?;
        match settled {
            None => self.session.keep(unit),
            Some(inputs) => {
                self.session.put(Unit { inputs, as_of: self.started, ..unit.clone() });
            }
        }
        Some(text)
    }

    /// Runs a plugin and, when results are being kept, records its value.
    fn execute(
        &self,
        dir: &Path,
        stage: Stage,
        plugin: &Plugin,
        source: String,
    ) -> Result<String, String> {
        let ran = run(dir, stage, plugin, &self.vars)?;
        if self.session.active()
            && let Some(body) = ran.value
        {
            let kind = UnitKind::Script;
            self.session.put(Unit { kind, source, as_of: self.started, inputs: ran.inputs, body });
        }
        Ok(ran.text)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache;
    use crate::script::value::Value;
    use crate::testing::{SECOND, Scratch};

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
        let text = run(scratch.root(), Stage::Pre, &plugins[0], &[]).unwrap().text;
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
            let vars = [("YIO_CACHE", cache.to_string())];
            run(scratch.root(), Stage::Pre, plugin, &vars).unwrap().text
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

    /// What `stage` would print for one `pre` plugin, and whether that came
    /// from the cache instead of from running it.
    fn shown(plugins: &Plugins, dir: &Path, plugin: &Plugin) -> (String, bool) {
        let source = show(&dir.join(".yio").join("pre").join(&plugin.file));
        match plugins.replay(&source, plugin) {
            Some(text) => (text, true),
            None => (plugins.execute(dir, Stage::Pre, plugin, source).unwrap(), false),
        }
    }

    fn ran(text: &str) -> (String, bool) {
        (text.to_string(), false)
    }

    fn replayed(text: &str) -> (String, bool) {
        (text.to_string(), true)
    }

    const LISTING: &str = r#"(map (list-dir "lore") (fn (f) (str f " " (file-size f))))"#;

    /// Runs the listing plugin once with nothing stored and returns the
    /// cache file that run would write, plus the time it ran.
    fn first_run(scratch: &Scratch) -> (Vec<u8>, u64) {
        scratch.write("lore/a.md", "x");
        scratch.write(".yio/pre/1_view.yio", LISTING);
        let plugin = &discover(scratch.root(), Stage::Pre).unwrap()[0];
        let started = input::now();
        let session = Session::from_file(None, true);
        let plugins = Plugins::new(Builtins::default(), &session, started);
        assert_eq!(shown(&plugins, scratch.root(), plugin), ran("lore/a.md 1\n"));
        (session.contents(), started)
    }

    #[test]
    fn a_yio_plugin_is_replayed_while_its_inputs_are_unchanged() {
        let scratch = Scratch::new("plugin-replay");
        let (file, started) = first_run(&scratch);
        let plugin = &discover(scratch.root(), Stage::Pre).unwrap()[0];
        let later = started + 10 * SECOND;

        let session = Session::from_file(Some(&file), false);
        let plugins = Plugins::new(Builtins::default(), &session, later);
        assert_eq!(shown(&plugins, scratch.root(), plugin), replayed("lore/a.md 1\n"));

        // Swapping the stored value shows the script really was not run.
        let mut units = cache::parse(&file).unwrap();
        units[0].body = encode::encode(&Value::str("from the cache")).unwrap();
        let doctored = cache::serialize(&units);
        let session = Session::from_file(Some(&doctored), false);
        let plugins = Plugins::new(Builtins::default(), &session, later);
        assert_eq!(shown(&plugins, scratch.root(), plugin), replayed("from the cache\n"));
    }

    #[test]
    fn a_yio_plugin_runs_again_when_something_it_read_changes() {
        let scratch = Scratch::new("plugin-rerun");
        let (file, started) = first_run(&scratch);
        let plugin = &discover(scratch.root(), Stage::Pre).unwrap()[0];
        let later = started + 10 * SECOND;
        let show_now = || {
            let session = Session::from_file(Some(&file), false);
            let plugins = Plugins::new(Builtins::default(), &session, later);
            let result = shown(&plugins, scratch.root(), plugin);
            (result, session.refreshed().map(|contents| cache::parse(&contents).unwrap()))
        };

        // A file appears in the directory it listed.
        scratch.write("lore/b.md", "xyz");
        let (result, written) = show_now();
        assert_eq!(result, ran("lore/a.md 1\nlore/b.md 3\n"));
        assert_eq!(written.unwrap().len(), 1);
        std::fs::remove_file(scratch.path("lore/b.md")).unwrap();
        assert_eq!(show_now().0, replayed("lore/a.md 1\n"));

        // A file it measured changes size.
        scratch.write("lore/a.md", "longer");
        assert_eq!(show_now().0, ran("lore/a.md 6\n"));
        scratch.write("lore/a.md", "x");

        // A file it never looked at changes.
        scratch.write("other.md", "irrelevant");
        assert_eq!(show_now().0, replayed("lore/a.md 1\n"));

        // The script itself is edited.
        scratch.write(".yio/pre/1_view.yio", r#""rewritten""#);
        assert_eq!(show_now().0, ran("rewritten\n"));
    }

    #[test]
    fn a_stored_value_is_not_reused_under_different_variables() {
        let scratch = Scratch::new("plugin-replay-vars");
        scratch.write(
            ".yio/pre/1_cache.yio",
            r#"(if (= (env "YIO_CACHE") "true") "caching" "not caching")"#,
        );
        let plugin = &discover(scratch.root(), Stage::Pre).unwrap()[0];
        let started = input::now();
        let caching = Builtins { cache: true, ..Builtins::default() };

        let session = Session::from_file(None, true);
        let plugins = Plugins::new(caching, &session, started);
        assert_eq!(shown(&plugins, scratch.root(), plugin), ran("caching\n"));
        let file = session.contents();

        // The value was computed with YIO_CACHE=true, so a normal run
        // cannot use it.
        let later = started + 10 * SECOND;
        let session = Session::from_file(Some(&file), false);
        let plugins = Plugins::new(Builtins::default(), &session, later);
        assert_eq!(shown(&plugins, scratch.root(), plugin), ran("not caching\n"));
        let file = session.refreshed().unwrap();

        let session = Session::from_file(Some(&file), false);
        let plugins = Plugins::new(Builtins::default(), &session, later);
        assert_eq!(shown(&plugins, scratch.root(), plugin), replayed("not caching\n"));
    }

    #[test]
    fn a_bash_plugin_is_never_replayed() {
        let scratch = Scratch::new("plugin-replay-bash");
        let plugin = Plugin::parse("1_sync.bash").unwrap();
        let source = show(&scratch.path(".yio/pre/1_sync.bash"));
        // Even with a stored value under its name that needs nothing.
        let unit = Unit {
            kind: UnitKind::Script,
            source: source.clone(),
            as_of: 0,
            inputs: Vec::new(),
            body: encode::encode(&Value::str("stored")).unwrap(),
        };
        let file = cache::serialize(&[unit]);
        let session = Session::from_file(Some(&file), false);
        let plugins = Plugins::new(Builtins::default(), &session, 0);
        assert_eq!(plugins.replay(&source, &plugin), None);

        let twin = Plugin { kind: Kind::Yio, ..plugin };
        assert_eq!(plugins.replay(&source, &twin), Some("stored\n".to_string()));
    }

    #[test]
    fn nothing_is_recorded_without_a_cache() {
        let scratch = Scratch::new("plugin-no-cache");
        scratch.write(".yio/pre/1_hello.yio", r#""hello""#);
        let plugin = &discover(scratch.root(), Stage::Pre).unwrap()[0];
        let session = Session::from_file(None, false);
        let plugins = Plugins::new(Builtins::default(), &session, input::now());
        assert_eq!(shown(&plugins, scratch.root(), plugin), ran("hello\n"));
        assert_eq!(cache::parse(&session.contents()).unwrap(), []);
    }

    #[test]
    fn pre_plugins_run_once_per_directory_and_not_for_version() {
        let scratch = Scratch::new("plugin-once");
        // A plugin that fails, so that running it is observable.
        scratch.write(".yio/pre/1_boom.yio", "(nope)");
        let session = Session::from_file(None, false);

        let plugins = Plugins::new(Builtins::default(), &session, 0);
        assert!(plugins.before(scratch.root()).is_err());
        assert!(plugins.before(scratch.root()).is_ok());

        for flags in [
            Builtins { version: true, ..Builtins::default() },
            Builtins { remove_cache: true, ..Builtins::default() },
        ] {
            let plugins = Plugins::new(flags, &session, 0);
            assert!(!plugins.enabled());
            assert!(plugins.before(scratch.root()).is_ok());
            assert!(plugins.after().is_ok());
        }
    }
}
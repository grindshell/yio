# yio

> Tasks for working on yio itself

This file is the project's own `yio.md`, and a working example of one. Run `yio` here to list the commands, and `yio COMMAND --help` to read about one.

The commands need `cargo` and `git`. On Windows they are written for Git Bash, so that has to be the first `bash` on `PATH`. The bash of WSL has its own programs and does not find the ones Windows has.

## build

> Build the `yio` binary

A debug build goes to `target/debug/`, and a release build to `target/release/`.

Windows cannot replace a program while it is running. So there, run this with a `yio` that is installed somewhere else, not with the one in `target/`.

**FLAGS**
- RELEASE
  - flags: `-r --release`
  - type: boolean
  - description: Build with optimizations

```bash
if [ "$RELEASE" = true ]; then
  cargo build --release
else
  cargo build
fi
```

## test [...NAMES]

> Run the tests, or only those with one of the words in their name

`yio test` runs them all. `yio test markdown cache` runs the tests of those two modules, since a test's name starts with its module.

```bash
cargo test -- "${NAMES[@]}"
```

## lint

> Check the code with clippy, as CI does

CI fails on any warning, so this does too.

```bash
cargo clippy --all-targets -- -D warnings
```

## check

> Do everything CI does: build, test, and lint

Run this before pushing. Each step is a code block of its own, so each gets a fresh shell, and the first one that fails stops the rest.

```bash
cargo build --locked
```

```bash
cargo test --locked
```

```bash
cargo clippy --all-targets --locked -- -D warnings
```

## run [...ARGS]

> Run `yio` from source with the arguments given

This builds what is in `src/` and runs it here, which is the quick way to try a change: `yio run docs search file`.

A flag would be taken as this command's own, so put `--` before the first one: `yio run -- --version`.

```bash
cargo run --quiet -- "${ARGS[@]}"
```

## release (VERSION)

> Tag a version, which is what starts a release

Give the version without the `v`: `yio release 0.2.0`. The version in `Cargo.toml` has to be that one already, and everything has to be committed. The release workflow builds the binaries once the tag is pushed.

**FLAGS**
- PUSH
  - flags: `--push`
  - type: boolean
  - description: Push the tag as well, instead of saying how

```bash
set -e

current="$(sed -n 's/^version = "\([^"]*\)".*/\1/p' Cargo.toml | head -n 1)"
if [ "$current" != "$VERSION" ]; then
  echo "Cargo.toml is at version $current, not $VERSION. Set it there and commit first." >&2
  exit 1
fi
if [ "$(git status --porcelain)" != "" ]; then
  echo "There are uncommitted changes. Commit or stash them first." >&2
  exit 1
fi

git tag "v$VERSION"
echo "Tagged v$VERSION."
if [ "$PUSH" = true ]; then
  git push origin "v$VERSION"
else
  echo "To start the release: git push origin v$VERSION"
fi
```

## docs

[docs](docs/yio.md)

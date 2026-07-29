# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

A Rust reimplementation of GNU findutils (`find`, `xargs`; `locate`/`updatedb` are goals but not yet present). Part of the [uutils](https://github.com/uutils) project. The aim is a full drop-in replacement matching GNU behavior and options.

> **Legal constraint (hard rule):** This is original code. Do NOT copy from or even look at the GNU source. Reference implementations under BSD/MIT (Apple's `file_cmds`, OpenBSD) are permitted. When matching behavior, consult the GNU *manual* (`info find`) or man pages, not the source.

## Common commands

```sh
cargo build                          # debug build; produces target/debug/{find,xargs}
cargo build --release                # optimized (thin LTO, single codegen unit)
cargo test                           # run all unit + integration tests
cargo test --test test_find          # run one integration test file (tests/test_find.rs)
cargo test test_name_matcher         # run tests matching a substring
cargo run --bin find -- . -name '*.rs'   # run find directly

cargo clippy --all-targets --all-features   # must be warning-free to merge
cargo fmt --all                              # rustfmt; must be clean to merge
cargo deny --all-features check all          # dependency/license checks
```

### Compatibility testing against real testsuites

```sh
bash util/build-gnu.sh                       # build + run the GNU findutils testsuite
bash util/build-gnu.sh tests/misc/help-version.sh   # run a single GNU test
bash util/build-bfs.sh                       # build + run the bfs testsuite
bash util/build-bfs.sh posix/basic           # run a single bfs test
```

`build-gnu.sh` expects the GNU source checked out at `../findutils.gnu` (clone from `git.savannah.gnu.org/git/findutils.git`). It copies the Rust `find`/`xargs` over the GNU binaries and runs `make check`. The `compare_*_result.py` / `diff-*.sh` scripts diff results against the tracked baseline.

## Architecture

Two binaries (`find`, `xargs`) plus a `testing-commandline` helper, all defined in `Cargo.toml` and backed by the `findutils` library (`src/lib.rs` → `find`, `xargs` modules). Binary `main.rs` files are thin: they collect args, build a dependency object, and call `find_main` / `xargs_main`.

### Dependency injection for testability

Both utilities take a `Dependencies` trait (`src/find/mod.rs`) abstracting output (`get_output() -> &RefCell<dyn Write>`) and the current time (`now()`). Production uses `StandardDependencies` (writes to stdout, real clock); tests use `FakeDependencies` (captures output to a string, fixed clock). This lets tests assert on exact output and makes time-based matchers deterministic. The test copy lives in `tests/common/test_helpers.rs`.

### find: the matcher tree (the core abstraction)

`find`'s expression engine is in `src/find/matchers/`. Everything is built around the `Matcher` trait (`matchers/mod.rs`):

- `matches(&self, entry: &WalkEntry, io: &mut MatcherIO) -> bool` — evaluate against a file entry.
- `has_side_effects()` — whether the matcher prints/acts (e.g. `-print`, `-exec`, `-delete`). If a parsed expression has no side effects, `build_top_level_matcher` wraps it in an implicit `-print`.
- `finished_dir()` / `finished()` — hooks for cleanup (e.g. `-depth`, `-execdir` batching).

Flow:
1. `parse_args` (`src/find/mod.rs`) strips leading global flags (`-H`/`-L`/`-P` follow modes, `-O[0-3]`, `--`), collects path operands, then hands the remaining args to the matcher builder.
2. `build_top_level_matcher` → `build_matcher_tree` recursively parses the expression, handling operator precedence: `(` `)` grouping, `!`/`-not`, implicit and explicit `-a`/`-and`, `-o`/`-or`, and `,` lists. Logical combinators live in `logical_matchers.rs`.
3. Each predicate is its own file/struct implementing `Matcher` — e.g. `name.rs`, `size.rs`, `time.rs`, `perm.rs`, `type_matcher.rs`, `regex.rs`, `printf.rs`, `exec.rs`, `delete.rs`, `prune.rs`. Adding a new predicate = new matcher struct + wiring a case into the parser in `mod.rs`.
4. `process_dir` walks the tree with `walkdir`, wrapping each result in a `WalkEntry` (`matchers/entry.rs`) that honors the `Follow` symlink mode, and evaluates the top matcher against each entry.

`MatcherIO` carries per-evaluation state (output handle, exit-code requests, "should quit" for `-quit`, deletion success tracking, etc.) threaded through the tree. `ComparableValue` (`EqualTo`/`LessThan`/`MoreThan`) models GNU's `N`/`-N`/`+N` numeric argument convention, parsed by the `convert_arg_to_comparable_value*` helpers.

### xargs

`src/xargs/mod.rs` is largely self-contained: an `Options` struct parsed from argv, then `process_input` reads stdin, splits into argument batches (respecting `-n`, `-L`, `-I`, `-0`, delimiter and size limits via the `argmax` crate), and spawns the command per batch.

### Tests

Unit tests live inline (`#[cfg(test)] mod tests`) in each matcher file. Integration tests are in `tests/` (`test_find.rs`, `test_xargs.rs`, `find_exec_tests.rs`, `exec_unit_tests.rs`) using fixtures under `test_data/` and helpers in `tests/common/`. The `testing-commandline` binary is used by exec-related tests as a controllable child process.

## Conventions

- Follow GNU option names/semantics; when GNU and the code disagree, GNU is the reference (via its manual, not its source).
- Spell-check (cspell) runs in CI — add `// spell-checker:ignore <word>` at the top of a file for intentional non-words.
- Clippy lints are configured in `Cargo.toml` (`[lints.clippy]`); MSRV-sensitive lints respect `clippy.toml`.

## Shared conventions

@~/dev/debian/coreutils-claude/common/coding-style.md
@~/dev/debian/coreutils-claude/common/benchmark.md
@~/dev/debian/coreutils-claude/common/ci.md
@~/dev/debian/coreutils-claude/common/testing.md
@~/dev/debian/coreutils-claude/common/code.md

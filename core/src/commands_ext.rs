//! Additional library-backed commands (policy: crate-backed, no reinventing).
//! Registered as brush builtins; each maps CLI flags to a crate and routes
//! bytes through the shell's fd table.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use brush_core::{builtins, ExecutionContext, ExecutionResult, ShellExtensions};
use clap::Parser;

fn abs(cwd: &Path, p: &str) -> PathBuf {
    let path = PathBuf::from(p);
    if path.is_absolute() {
        path
    } else {
        cwd.join(path)
    }
}

/// Shell-style glob (`*`, `?`) as an anchored regex. Used by the flags that
/// take a name pattern rather than a path — `tar --exclude`, `zip -x`,
/// `unzip -x`, `tree -P/-I`.
fn glob_to_regex(glob: &str) -> regex_lite::Regex {
    let mut re = String::from("^");
    for ch in glob.chars() {
        match ch {
            '*' => re.push_str(".*"),
            '?' => re.push('.'),
            '.' | '+' | '(' | ')' | '|' | '[' | ']' | '{' | '}' | '^' | '$' | '\\' => {
                re.push('\\');
                re.push(ch);
            }
            _ => re.push(ch),
        }
    }
    re.push('$');
    regex_lite::Regex::new(&re).unwrap_or_else(|_| regex_lite::Regex::new("^$").unwrap())
}

// ---------------------------------------------------------------- which

/// Locate a command: reports shell builtins and PATH executables.
#[derive(Parser)]
pub struct WhichCommand {
    /// Print every match on PATH, not just the first.
    #[arg(short = 'a', long = "all")]
    all: bool,
    /// Print nothing; the exit status alone reports whether all names resolved.
    #[arg(short = 's', long = "silent")]
    silent: bool,
    /// Ignore shell builtins and functions; only look on PATH.
    #[arg(long = "skip-functions")]
    skip_functions: bool,
    names: Vec<String>,
}

impl WhichCommand {
    /// Every executable named `name` on PATH, in order. brush's lookup cache
    /// only answers "the first one", which is not enough for `-a`.
    fn all_on_path<SE: ShellExtensions>(
        context: &ExecutionContext<'_, SE>,
        name: &str,
    ) -> Vec<PathBuf> {
        let Some(path_var) = context.shell.env().get("PATH") else {
            return Vec::new();
        };
        path_var
            .1
            .value()
            .to_cow_str(context.shell)
            .split(':')
            .filter(|d| !d.is_empty())
            .map(|d| PathBuf::from(d).join(name))
            .filter(|p| p.is_file())
            .collect()
    }
}

impl builtins::Command for WhichCommand {
    type Error = brush_core::Error;
    async fn execute<SE: ShellExtensions>(
        &self,
        context: ExecutionContext<'_, SE>,
    ) -> Result<ExecutionResult, Self::Error> {
        let mut out = context.stdout();
        let mut exit = 0u8;
        for name in &self.names {
            let is_shell_word = !self.skip_functions
                && (context.shell.builtins().contains_key(name)
                    || context.shell.funcs().get(name.as_str()).is_some());
            if is_shell_word {
                if !self.silent {
                    writeln!(out, "{name}: shell builtin")?;
                }
                continue;
            }
            let hits: Vec<PathBuf> = if self.all {
                Self::all_on_path(&context, name)
            } else {
                context
                    .shell
                    .find_first_executable_in_path_using_cache(name)
                    .into_iter()
                    .collect()
            };
            if hits.is_empty() {
                if !self.silent {
                    writeln!(context.stderr(), "{name} not found")?;
                }
                exit = 1;
            } else if !self.silent {
                for p in hits {
                    writeln!(out, "{}", p.display())?;
                }
            }
        }
        out.flush()?;
        Ok(ExecutionResult::new(exit))
    }
}

// ---------------------------------------------------------------- tree

/// Print a directory tree.
#[derive(Parser)]
pub struct TreeCommand {
    /// Root directory (default: current).
    path: Option<String>,
    /// Show hidden entries.
    #[arg(short = 'a', long = "all")]
    all: bool,
    /// Descend at most this many levels.
    #[arg(short = 'L', value_name = "LEVEL")]
    level: Option<usize>,
    /// List directories only.
    #[arg(short = 'd')]
    dirs_only: bool,
    /// Print the full path prefix for each entry.
    #[arg(short = 'f')]
    full_path: bool,
    /// Do not indent; useful with -f.
    #[arg(short = 'i')]
    no_indent: bool,
    /// List only entries matching this glob.
    #[arg(short = 'P', value_name = "PATTERN")]
    pattern: Option<String>,
    /// Skip entries matching this glob.
    #[arg(short = 'I', value_name = "PATTERN")]
    ignore: Option<String>,
}

/// Display options threaded through the recursion, kept in a struct so adding
/// one does not mean touching every call site.
#[derive(Clone)]
struct TreeOpts {
    all: bool,
    max: Option<usize>,
    dirs_only: bool,
    full_path: bool,
    no_indent: bool,
    pattern: Option<regex_lite::Regex>,
    ignore: Option<regex_lite::Regex>,
}

impl builtins::Command for TreeCommand {
    type Error = brush_core::Error;
    async fn execute<SE: ShellExtensions>(
        &self,
        context: ExecutionContext<'_, SE>,
    ) -> Result<ExecutionResult, Self::Error> {
        let cwd = context.shell.working_dir().to_path_buf();
        let root = abs(&cwd, self.path.as_deref().unwrap_or("."));
        let mut out = context.stdout();
        let root_label = self.path.as_deref().unwrap_or(".");
        writeln!(out, "{root_label}")?;
        let (mut dirs, mut files) = (0usize, 0usize);
        let opts = TreeOpts {
            all: self.all,
            max: self.level,
            dirs_only: self.dirs_only,
            full_path: self.full_path,
            no_indent: self.no_indent,
            pattern: self.pattern.as_ref().map(|g| glob_to_regex(g)),
            ignore: self.ignore.as_ref().map(|g| glob_to_regex(g)),
        };
        tree_walk(
            &root, "", root_label, &opts, 1, &mut out, &mut dirs, &mut files,
        )?;
        writeln!(out, "\n{dirs} directories, {files} files")?;
        out.flush()?;
        Ok(ExecutionResult::success())
    }
}

#[allow(clippy::too_many_arguments)]
fn tree_walk(
    dir: &Path,
    prefix: &str,
    path_prefix: &str,
    opts: &TreeOpts,
    depth: usize,
    out: &mut impl Write,
    dirs: &mut usize,
    files: &mut usize,
) -> std::io::Result<()> {
    if let Some(m) = opts.max {
        if depth > m {
            return Ok(());
        }
    }
    let mut entries: Vec<_> = match std::fs::read_dir(dir) {
        Ok(rd) => rd.flatten().collect(),
        Err(_) => return Ok(()),
    };
    entries.retain(|e| opts.all || !e.file_name().to_string_lossy().starts_with('.'));
    entries.sort_by_key(std::fs::DirEntry::file_name);
    let n = entries.len();
    for (i, entry) in entries.iter().enumerate() {
        let last = i + 1 == n;
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        // -I always prunes; -P and -d only filter what is PRINTED, since a
        // directory still has to be descended to reach matching entries.
        let ignored = opts.ignore.as_ref().is_some_and(|re| re.is_match(&name));
        if ignored {
            continue;
        }
        let printable = !(opts.dirs_only && !is_dir)
            && (is_dir || opts.pattern.as_ref().is_none_or(|re| re.is_match(&name)));
        let full = if path_prefix.is_empty() {
            name.clone()
        } else {
            format!("{path_prefix}/{name}")
        };
        if printable {
            let label = if opts.full_path { &full } else { &name };
            if opts.no_indent {
                writeln!(out, "{label}")?;
            } else {
                let connector = if last { "└── " } else { "├── " };
                writeln!(out, "{prefix}{connector}{label}")?;
            }
        }
        if is_dir {
            *dirs += 1;
            let child_prefix = if opts.no_indent {
                String::new()
            } else {
                format!("{prefix}{}", if last { "    " } else { "│   " })
            };
            tree_walk(
                &entry.path(),
                &child_prefix,
                &full,
                opts,
                depth + 1,
                out,
                dirs,
                files,
            )?;
        } else if printable {
            *files += 1;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- zip / unzip

/// Create a zip archive.
#[derive(Parser)]
pub struct ZipCommand {
    /// Recurse into directories.
    #[arg(short = 'r', long = "recurse-paths")]
    recurse: bool,
    /// Quiet: do not list each file as it is added.
    #[arg(short = 'q', long = "quiet")]
    quiet: bool,
    /// Store without compressing.
    #[arg(short = '0')]
    store: bool,
    /// Maximum compression. Accepted; deflate is already the strongest method
    /// the `zip` crate exposes here.
    #[arg(short = '9')]
    _best: bool,
    /// Junk paths: store only the basename of each entry.
    #[arg(short = 'j', long = "junk-paths")]
    junk_paths: bool,
    /// Delete the named entries from an existing archive.
    #[arg(short = 'd', long = "delete")]
    delete: bool,
    /// Update: keep existing entries and add/replace the named ones.
    #[arg(short = 'u', long = "update")]
    update: bool,
    /// Exclude entries matching this glob; repeatable.
    #[arg(short = 'x', long = "exclude", value_name = "PATTERN")]
    exclude: Vec<String>,
    /// Read file names from standard input.
    #[arg(short = '@', long = "names-stdin")]
    names_stdin: bool,
    /// Archive name.
    archive: String,
    /// Files/dirs to add (with -d, entry names to remove).
    paths: Vec<String>,
}

impl ZipCommand {
    fn excluded(&self, name: &str) -> bool {
        !self.exclude.is_empty() && self.exclude.iter().any(|g| glob_to_regex(g).is_match(name))
    }
}

impl builtins::Command for ZipCommand {
    type Error = brush_core::Error;
    async fn execute<SE: ShellExtensions>(
        &self,
        context: ExecutionContext<'_, SE>,
    ) -> Result<ExecutionResult, Self::Error> {
        let cwd = context.shell.working_dir().to_path_buf();
        let archive = abs(&cwd, &self.archive);
        let mut out = context.stdout();
        let mut paths = self.paths.clone();
        if self.names_stdin {
            let mut input = Vec::new();
            context.stdin().read_to_end(&mut input)?;
            paths.extend(
                String::from_utf8_lossy(&input)
                    .lines()
                    .filter(|line| !line.is_empty())
                    .map(str::to_owned),
            );
        }

        // -d and -u both need the existing entries. The `zip` crate has no
        // in-place edit, so read the old archive into memory and write a fresh
        // one — correct, and archives here are small enough for it.
        let mut carried: Vec<(String, Vec<u8>)> = Vec::new();
        if (self.delete || self.update) && archive.exists() {
            let f = std::fs::File::open(&archive)?;
            let mut zr = zip::ZipArchive::new(f).map_err(std::io::Error::other)?;
            for i in 0..zr.len() {
                let mut e = zr.by_index(i).map_err(std::io::Error::other)?;
                let name = e.name().to_string();
                // -d drops the named entries; -u drops the ones about to be
                // re-added, so the new copy wins.
                let dropped = if self.delete {
                    paths
                        .iter()
                        .any(|p| glob_to_regex(p).is_match(&name) || *p == name)
                } else {
                    paths.iter().any(|p| *p == name)
                };
                if dropped {
                    if self.delete && !self.quiet {
                        writeln!(out, "deleting: {name}")?;
                    }
                    continue;
                }
                let mut buf = Vec::new();
                e.read_to_end(&mut buf)?;
                carried.push((name, buf));
            }
        }

        let f = std::fs::File::create(&archive)?;
        let mut zw = zip::ZipWriter::new(f);
        let method = if self.store {
            zip::CompressionMethod::Stored
        } else {
            zip::CompressionMethod::Deflated
        };
        let opts: zip::write::FileOptions<()> =
            zip::write::FileOptions::default().compression_method(method);

        for (name, data) in &carried {
            zw.start_file(name, opts).map_err(std::io::Error::other)?;
            zw.write_all(data)?;
        }

        if !self.delete {
            for p in &paths {
                let full = abs(&cwd, p);
                if full.is_dir() && self.recurse {
                    for entry in walkdir::WalkDir::new(&full).into_iter().flatten() {
                        if !entry.file_type().is_file() {
                            continue;
                        }
                        let rel = entry.path().strip_prefix(&cwd).unwrap_or(entry.path());
                        let name = if self.junk_paths {
                            entry.file_name().to_string_lossy().into_owned()
                        } else {
                            rel.to_string_lossy().into_owned()
                        };
                        if self.excluded(&name) {
                            continue;
                        }
                        zw.start_file(&name, opts).map_err(std::io::Error::other)?;
                        zw.write_all(&std::fs::read(entry.path())?)?;
                        if !self.quiet {
                            writeln!(out, "  adding: {name}")?;
                        }
                    }
                } else if full.is_file() {
                    let name = if self.junk_paths {
                        full.file_name()
                            .map_or_else(|| p.clone(), |n| n.to_string_lossy().into_owned())
                    } else {
                        p.clone()
                    };
                    if self.excluded(&name) {
                        continue;
                    }
                    zw.start_file(&name, opts).map_err(std::io::Error::other)?;
                    zw.write_all(&std::fs::read(&full)?)?;
                    if !self.quiet {
                        writeln!(out, "  adding: {name}")?;
                    }
                }
            }
        }
        zw.finish().map_err(std::io::Error::other)?;
        out.flush()?;
        Ok(ExecutionResult::success())
    }
}

// ---------------------------------------------------------------- jq

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abs_preserves_absolute_and_resolves_relative_paths() {
        let cwd = Path::new("/work");
        assert_eq!(abs(cwd, "/tmp/file"), PathBuf::from("/tmp/file"));
        assert_eq!(abs(cwd, "file"), PathBuf::from("/work/file"));
    }

    #[test]
    fn shell_glob_supports_wildcards_and_escapes_regex_metacharacters() {
        let wildcard = glob_to_regex("a?.*");
        assert!(wildcard.is_match("ab.txt"));
        assert!(!wildcard.is_match("a.txt"));

        let literal = glob_to_regex("a+[b](c){d}|^$\\");
        assert!(literal.is_match("a+[b](c){d}|^$\\"));
        assert!(!literal.is_match("aaab"));
    }

    #[test]
    fn tree_walk_handles_empty_prefix_depth_limit_and_unreadable_root() {
        let temp = std::env::temp_dir().join(format!(
            "yourshell-tree-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(temp.join("dir")).unwrap();
        std::fs::write(temp.join("file"), b"x").unwrap();

        let opts = TreeOpts {
            all: true,
            max: Some(0),
            dirs_only: false,
            full_path: true,
            no_indent: true,
            pattern: None,
            ignore: None,
        };
        let mut output = Vec::new();
        let (mut dirs, mut files) = (0, 0);
        tree_walk(&temp, "", "", &opts, 1, &mut output, &mut dirs, &mut files).unwrap();
        assert!(output.is_empty());

        let mut output = Vec::new();
        tree_walk(
            &temp.join("missing"),
            "",
            "",
            &TreeOpts { max: None, ..opts },
            1,
            &mut output,
            &mut dirs,
            &mut files,
        )
        .unwrap();
        assert!(output.is_empty());
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn zip_exclusion_is_disabled_when_empty_and_matches_any_pattern() {
        let base = ZipCommand {
            recurse: false,
            update: false,
            delete: false,
            store: false,
            _best: false,
            junk_paths: false,
            quiet: false,
            names_stdin: false,
            exclude: Vec::new(),
            archive: "out.zip".into(),
            paths: Vec::new(),
        };
        assert!(!base.excluded("skip.tmp"));

        let matching = ZipCommand {
            exclude: vec!["*.tmp".into(), "exact".into()],
            ..base
        };
        assert!(matching.excluded("skip.tmp"));
        assert!(matching.excluded("exact"));
        assert!(!matching.excluded("keep.txt"));
    }
}

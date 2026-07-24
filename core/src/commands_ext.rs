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

// ---------------------------------------------------------------- which

/// Locate a command: reports shell builtins and PATH executables.
#[derive(Parser)]
pub struct WhichCommand {
    names: Vec<String>,
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
            if context.shell.builtins().contains_key(name)
                || context.shell.funcs().get(name.as_str()).is_some()
            {
                writeln!(out, "{name}: shell builtin")?;
            } else if let Some(path) = context
                .shell
                .find_first_executable_in_path_using_cache(name)
            {
                writeln!(out, "{}", path.display())?;
            } else {
                writeln!(context.stderr(), "{name} not found")?;
                exit = 1;
            }
        }
        out.flush()?;
        Ok(ExecutionResult::new(exit))
    }
}

// ---------------------------------------------------------------- find

/// Walk a directory tree, printing paths matching simple predicates.
#[derive(Parser)]
pub struct FindCommand {
    /// Roots to search (default: current directory).
    paths: Vec<String>,
    /// Match entries whose name matches this glob (`-name`).
    #[arg(long = "name")]
    name: Option<String>,
    /// Match type: f (file), d (dir), l (symlink) (`-type`).
    #[arg(long = "type")]
    typ: Option<char>,
    /// Descend at most this many directory levels (`-maxdepth`).
    #[arg(long = "maxdepth")]
    maxdepth: Option<usize>,
}

impl builtins::Command for FindCommand {
    type Error = brush_core::Error;
    async fn execute<SE: ShellExtensions>(
        &self,
        context: ExecutionContext<'_, SE>,
    ) -> Result<ExecutionResult, Self::Error> {
        let cwd = context.shell.working_dir().to_path_buf();
        let roots = if self.paths.is_empty() {
            vec![".".to_string()]
        } else {
            self.paths.clone()
        };
        let mut out = context.stdout();
        let pattern = self.name.as_ref().map(|g| glob_to_regex(g));

        for root_str in &roots {
            let root = abs(&cwd, root_str);
            let mut walker = walkdir::WalkDir::new(&root);
            if let Some(d) = self.maxdepth {
                walker = walker.max_depth(d);
            }
            for entry in walker.into_iter().flatten() {
                let ft = entry.file_type();
                if let Some(t) = self.typ {
                    let ok = match t {
                        'f' => ft.is_file(),
                        'd' => ft.is_dir(),
                        'l' => ft.is_symlink(),
                        _ => true,
                    };
                    if !ok {
                        continue;
                    }
                }
                if let Some(re) = &pattern {
                    let fname = entry.file_name().to_string_lossy();
                    if !re.is_match(&fname) {
                        continue;
                    }
                }
                // Print relative to the given root string when possible.
                let disp = entry
                    .path()
                    .strip_prefix(&cwd)
                    .map(|p| {
                        if p.as_os_str().is_empty() {
                            root_str.clone()
                        } else {
                            p.display().to_string()
                        }
                    })
                    .unwrap_or_else(|_| entry.path().display().to_string());
                writeln!(out, "{disp}")?;
            }
        }
        out.flush()?;
        Ok(ExecutionResult::success())
    }
}

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

// ---------------------------------------------------------------- tree

/// Print a directory tree.
#[derive(Parser)]
pub struct TreeCommand {
    /// Root directory (default: current).
    path: Option<String>,
    /// Show hidden entries.
    #[arg(short = 'a')]
    all: bool,
    /// Descend at most this many levels.
    #[arg(short = 'L')]
    level: Option<usize>,
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
        writeln!(out, "{}", self.path.as_deref().unwrap_or("."))?;
        let (mut dirs, mut files) = (0usize, 0usize);
        tree_walk(
            &root,
            "",
            self.all,
            self.level,
            1,
            &mut out,
            &mut dirs,
            &mut files,
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
    all: bool,
    max: Option<usize>,
    depth: usize,
    out: &mut impl Write,
    dirs: &mut usize,
    files: &mut usize,
) -> std::io::Result<()> {
    if let Some(m) = max {
        if depth > m {
            return Ok(());
        }
    }
    let mut entries: Vec<_> = match std::fs::read_dir(dir) {
        Ok(rd) => rd.flatten().collect(),
        Err(_) => return Ok(()),
    };
    entries.retain(|e| all || !e.file_name().to_string_lossy().starts_with('.'));
    entries.sort_by_key(std::fs::DirEntry::file_name);
    let n = entries.len();
    for (i, entry) in entries.iter().enumerate() {
        let last = i + 1 == n;
        let connector = if last { "└── " } else { "├── " };
        let name = entry.file_name().to_string_lossy().into_owned();
        writeln!(out, "{prefix}{connector}{name}")?;
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            *dirs += 1;
            let child_prefix = format!("{prefix}{}", if last { "    " } else { "│   " });
            tree_walk(
                &entry.path(),
                &child_prefix,
                all,
                max,
                depth + 1,
                out,
                dirs,
                files,
            )?;
        } else {
            *files += 1;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- diff

/// Compare two files, producing a unified diff.
#[derive(Parser)]
pub struct DiffCommand {
    a: String,
    b: String,
    /// Number of context lines.
    #[arg(short = 'U', default_value = "3")]
    context: usize,
}

impl builtins::Command for DiffCommand {
    type Error = brush_core::Error;
    async fn execute<SE: ShellExtensions>(
        &self,
        context: ExecutionContext<'_, SE>,
    ) -> Result<ExecutionResult, Self::Error> {
        let cwd = context.shell.working_dir().to_path_buf();
        let ta = std::fs::read_to_string(abs(&cwd, &self.a)).unwrap_or_default();
        let tb = std::fs::read_to_string(abs(&cwd, &self.b)).unwrap_or_default();
        let diff = similar::TextDiff::from_lines(&ta, &tb);
        let mut out = context.stdout();
        if diff.ratio() == 1.0 {
            return Ok(ExecutionResult::success());
        }
        let ud = diff
            .unified_diff()
            .context_radius(self.context)
            .header(&self.a, &self.b)
            .to_string();
        write!(out, "{ud}")?;
        out.flush()?;
        Ok(ExecutionResult::new(1)) // differences found
    }
}

// ---------------------------------------------------------------- gzip / gunzip

/// Compress a file with gzip.
#[derive(Parser)]
pub struct GzipCommand {
    /// Write to stdout, keep input.
    #[arg(short = 'c')]
    stdout: bool,
    /// Decompress (same as gunzip).
    #[arg(short = 'd')]
    decompress: bool,
    /// Keep the input file.
    #[arg(short = 'k')]
    keep: bool,
    files: Vec<String>,
}

impl builtins::Command for GzipCommand {
    type Error = brush_core::Error;
    async fn execute<SE: ShellExtensions>(
        &self,
        context: ExecutionContext<'_, SE>,
    ) -> Result<ExecutionResult, Self::Error> {
        gzip_run(self.decompress, self.stdout, self.keep, &self.files, &context)
    }
}

/// Decompress a gzip file.
#[derive(Parser)]
pub struct GunzipCommand {
    #[arg(short = 'c')]
    stdout: bool,
    #[arg(short = 'k')]
    keep: bool,
    files: Vec<String>,
}

impl builtins::Command for GunzipCommand {
    type Error = brush_core::Error;
    async fn execute<SE: ShellExtensions>(
        &self,
        context: ExecutionContext<'_, SE>,
    ) -> Result<ExecutionResult, Self::Error> {
        gzip_run(true, self.stdout, self.keep, &self.files, &context)
    }
}

fn gzip_run<SE: ShellExtensions>(
    decompress: bool,
    to_stdout: bool,
    keep: bool,
    files: &[String],
    context: &ExecutionContext<'_, SE>,
) -> Result<ExecutionResult, brush_core::Error> {
    use flate2::read::GzDecoder;
    use flate2::write::GzEncoder;
    use flate2::Compression;
    let cwd = context.shell.working_dir().to_path_buf();
    let mut stdout = context.stdout();

    // stdin -> stdout streaming when no files.
    if files.is_empty() {
        let mut input = Vec::new();
        context.stdin().read_to_end(&mut input)?;
        let output = if decompress {
            let mut d = GzDecoder::new(&input[..]);
            let mut buf = Vec::new();
            d.read_to_end(&mut buf)?;
            buf
        } else {
            let mut e = GzEncoder::new(Vec::new(), Compression::default());
            e.write_all(&input)?;
            e.finish()?
        };
        stdout.write_all(&output)?;
        stdout.flush()?;
        return Ok(ExecutionResult::success());
    }

    let mut stderr = context.stderr();
    let mut had_error = false;
    for f in files {
        let path = abs(&cwd, f);
        // Read the input; a read failure must NOT be treated as empty data (that
        // would write an empty output and then delete the original).
        let data = match std::fs::read(&path) {
            Ok(d) => d,
            Err(e) => {
                let _ = writeln!(stderr, "gzip: {}: {e}", path.display());
                had_error = true;
                continue;
            }
        };
        let (output, out_path) = if decompress {
            // Only decompress files that actually end in `.gz`; otherwise the
            // output path would equal the input path and we'd overwrite then
            // delete the source.
            let Some(stripped) = path.to_string_lossy().strip_suffix(".gz").map(str::to_string)
            else {
                let _ = writeln!(stderr, "gzip: {}: unknown suffix -- ignored", path.display());
                had_error = true;
                continue;
            };
            let mut d = GzDecoder::new(&data[..]);
            let mut buf = Vec::new();
            if let Err(e) = d.read_to_end(&mut buf) {
                let _ = writeln!(stderr, "gzip: {}: not in gzip format: {e}", path.display());
                had_error = true;
                continue;
            }
            (buf, stripped)
        } else {
            let mut e = GzEncoder::new(Vec::new(), Compression::default());
            e.write_all(&data)?;
            (e.finish()?, format!("{}.gz", path.to_string_lossy()))
        };
        if to_stdout {
            stdout.write_all(&output)?;
        } else if let Err(e) = std::fs::write(&out_path, &output) {
            let _ = writeln!(stderr, "gzip: {out_path}: {e}");
            had_error = true;
        } else if !keep {
            // Only remove the source after a confirmed successful write.
            let _ = std::fs::remove_file(&path);
        }
    }
    stdout.flush()?;
    Ok(ExecutionResult::new(u8::from(had_error)))
}

// ---------------------------------------------------------------- sed

/// Stream editor: applies `s/re/rep/flags` (and multiple `-e`) via sedregex.
#[derive(Parser)]
pub struct SedCommand {
    /// Suppress automatic printing (only relevant with p flag).
    #[arg(short = 'n')]
    quiet: bool,
    /// A sed expression (repeatable).
    #[arg(short = 'e')]
    exprs: Vec<String>,
    /// Positional: first non-option is the script if no -e given, rest are files.
    rest: Vec<String>,
}

impl builtins::Command for SedCommand {
    type Error = brush_core::Error;
    async fn execute<SE: ShellExtensions>(
        &self,
        context: ExecutionContext<'_, SE>,
    ) -> Result<ExecutionResult, Self::Error> {
        let cwd = context.shell.working_dir().to_path_buf();
        let mut cmds = self.exprs.clone();
        let mut files = self.rest.clone();
        if cmds.is_empty() && !files.is_empty() {
            cmds.push(files.remove(0));
        }
        let input = if files.is_empty() {
            let mut s = String::new();
            context.stdin().read_to_string(&mut s)?;
            s
        } else {
            files
                .iter()
                .filter_map(|f| std::fs::read_to_string(abs(&cwd, f)).ok())
                .collect::<Vec<_>>()
                .join("")
        };

        // GNU sed is line-oriented: each s/// (without a global flag) affects
        // the first match on each line. sedregex operates on a whole string,
        // so apply it per line and preserve the trailing newline layout.
        let had_trailing_nl = input.ends_with('\n');
        let mut out = context.stdout();
        let mut result = String::new();
        let parts: Vec<&str> = input.split('\n').collect();
        let n = parts.len();
        for (i, line) in parts.iter().enumerate() {
            if i > 0 {
                result.push('\n');
            }
            // Only the FINAL split item is the trailing-newline artifact (empty
            // iff the input ended in \n); the '\n' pushed above already preserved
            // that trailing newline. A *real* interior empty line must still be
            // run through the expression (e.g. `sed 's/^/X/'` outputs `X`).
            if i + 1 == n && had_trailing_nl && line.is_empty() {
                continue;
            }
            match sedregex::find_and_replace(line, &cmds) {
                Ok(r) => result.push_str(&r),
                Err(e) => {
                    writeln!(context.stderr(), "sed: {e:?}")?;
                    return Ok(ExecutionResult::new(1));
                }
            }
        }
        if !self.quiet {
            write!(out, "{result}")?;
        }
        out.flush()?;
        Ok(ExecutionResult::success())
    }
}

// ---------------------------------------------------------------- curl / wget

/// Transfer a URL (HTTP/HTTPS GET), printing the body to stdout.
#[derive(Parser)]
pub struct CurlCommand {
    url: String,
    /// Write output to this file instead of stdout.
    #[arg(short = 'o')]
    output: Option<String>,
    /// Follow the remote name (save as the URL's basename).
    #[arg(short = 'O')]
    remote_name: bool,
    /// Silent: no error text.
    #[arg(short = 's')]
    silent: bool,
    /// Include headers (ignored beyond status) / fail flag placeholder.
    #[arg(short = 'L', default_value_t = true)]
    _location: bool,
}

impl builtins::Command for CurlCommand {
    type Error = brush_core::Error;
    async fn execute<SE: ShellExtensions>(
        &self,
        context: ExecutionContext<'_, SE>,
    ) -> Result<ExecutionResult, Self::Error> {
        let cwd = context.shell.working_dir().to_path_buf();
        let dest = if self.remote_name {
            Some(url_basename(&self.url))
        } else {
            self.output.clone()
        };
        http_get(&self.url, dest.as_deref(), self.silent, &cwd, &context)
    }
}

/// Retrieve a URL, saving to a local file (like wget).
#[derive(Parser)]
pub struct WgetCommand {
    url: String,
    /// Output file (default: URL basename).
    #[arg(short = 'O')]
    output: Option<String>,
    /// Write to stdout.
    #[arg(short = 'q')]
    quiet: bool,
    /// Print to stdout instead of saving.
    #[arg(long = "stdout")]
    to_stdout: bool,
}

impl builtins::Command for WgetCommand {
    type Error = brush_core::Error;
    async fn execute<SE: ShellExtensions>(
        &self,
        context: ExecutionContext<'_, SE>,
    ) -> Result<ExecutionResult, Self::Error> {
        let cwd = context.shell.working_dir().to_path_buf();
        let dest = if self.to_stdout {
            None
        } else {
            Some(self.output.clone().unwrap_or_else(|| url_basename(&self.url)))
        };
        http_get(&self.url, dest.as_deref(), self.quiet, &cwd, &context)
    }
}

fn url_basename(url: &str) -> String {
    let no_query = url.split('?').next().unwrap_or(url);
    let base = no_query.rsplit('/').next().unwrap_or("index.html");
    if base.is_empty() {
        "index.html".to_string()
    } else {
        base.to_string()
    }
}

fn http_get<SE: ShellExtensions>(
    url: &str,
    dest: Option<&str>,
    silent: bool,
    cwd: &Path,
    context: &ExecutionContext<'_, SE>,
) -> Result<ExecutionResult, brush_core::Error> {
    match ureq::get(url).call() {
        Ok(mut resp) => {
            // Stream the body straight to the destination so a large download
            // doesn't buffer the whole response in memory (OOM on iOS).
            let mut reader = resp.body_mut().as_reader();
            let copy_result = if let Some(d) = dest {
                let path = abs(cwd, d);
                match std::fs::File::create(&path) {
                    Ok(mut f) => std::io::copy(&mut reader, &mut f).map(|_| ()),
                    Err(e) => {
                        if !silent {
                            let _ = writeln!(context.stderr(), "curl: {}: {e}", path.display());
                        }
                        return Ok(ExecutionResult::new(1));
                    }
                }
            } else {
                let mut out = context.stdout();
                let r = std::io::copy(&mut reader, &mut out).map(|_| ());
                let _ = out.flush();
                r
            };
            match copy_result {
                Ok(()) => Ok(ExecutionResult::success()),
                Err(e) => {
                    if !silent {
                        let _ = writeln!(context.stderr(), "curl: read error: {e}");
                    }
                    Ok(ExecutionResult::new(1))
                }
            }
        }
        Err(e) => {
            if !silent {
                let _ = writeln!(context.stderr(), "curl: {e}");
            }
            Ok(ExecutionResult::new(1))
        }
    }
}

// ---------------------------------------------------------------- tar

/// Create or extract tar archives (optionally gzip-compressed with -z).
#[derive(Parser)]
pub struct TarCommand {
    /// Create an archive.
    #[arg(short = 'c')]
    create: bool,
    /// Extract an archive.
    #[arg(short = 'x')]
    extract: bool,
    /// List archive contents.
    #[arg(short = 't')]
    list: bool,
    /// Filter through gzip.
    #[arg(short = 'z')]
    gzip: bool,
    /// Verbose.
    #[arg(short = 'v')]
    verbose: bool,
    /// Archive file.
    #[arg(short = 'f')]
    file: String,
    /// Files/dirs (for create).
    paths: Vec<String>,
}

impl builtins::Command for TarCommand {
    type Error = brush_core::Error;
    async fn execute<SE: ShellExtensions>(
        &self,
        context: ExecutionContext<'_, SE>,
    ) -> Result<ExecutionResult, Self::Error> {
        use flate2::read::GzDecoder;
        use flate2::write::GzEncoder;
        use flate2::Compression;
        let cwd = context.shell.working_dir().to_path_buf();
        let archive = abs(&cwd, &self.file);
        let mut out = context.stdout();

        if self.create {
            let f = std::fs::File::create(&archive)?;
            if self.gzip {
                let enc = GzEncoder::new(f, Compression::default());
                let mut builder = tar::Builder::new(enc);
                for p in &self.paths {
                    add_to_tar(&mut builder, &cwd, p, self.verbose, &mut out)?;
                }
                builder.into_inner()?.finish()?;
            } else {
                let mut builder = tar::Builder::new(f);
                for p in &self.paths {
                    add_to_tar(&mut builder, &cwd, p, self.verbose, &mut out)?;
                }
                builder.finish()?;
            }
        } else if self.extract || self.list {
            let f = std::fs::File::open(&archive)?;
            let reader: Box<dyn Read> = if self.gzip {
                Box::new(GzDecoder::new(f))
            } else {
                Box::new(f)
            };
            let mut ar = tar::Archive::new(reader);
            if self.list {
                for entry in ar.entries()? {
                    let e = entry?;
                    writeln!(out, "{}", e.path()?.display())?;
                }
            } else {
                for entry in ar.entries()? {
                    let mut e = entry?;
                    if self.verbose {
                        writeln!(out, "{}", e.path()?.display())?;
                    }
                    e.unpack_in(&cwd)?;
                }
            }
        } else {
            writeln!(context.stderr(), "tar: specify -c, -x or -t")?;
            return Ok(ExecutionResult::new(2));
        }
        out.flush()?;
        Ok(ExecutionResult::success())
    }
}

fn add_to_tar<W: Write>(
    builder: &mut tar::Builder<W>,
    cwd: &Path,
    p: &str,
    verbose: bool,
    out: &mut impl Write,
) -> std::io::Result<()> {
    let full = abs(cwd, p);
    if verbose {
        let _ = writeln!(out, "{p}");
    }
    if full.is_dir() {
        builder.append_dir_all(p, &full)?;
    } else {
        builder.append_path_with_name(&full, p)?;
    }
    Ok(())
}

// ---------------------------------------------------------------- zip / unzip

/// Create a zip archive.
#[derive(Parser)]
pub struct ZipCommand {
    /// Recurse into directories.
    #[arg(short = 'r')]
    recurse: bool,
    /// Archive name.
    archive: String,
    /// Files/dirs to add.
    paths: Vec<String>,
}

impl builtins::Command for ZipCommand {
    type Error = brush_core::Error;
    async fn execute<SE: ShellExtensions>(
        &self,
        context: ExecutionContext<'_, SE>,
    ) -> Result<ExecutionResult, Self::Error> {
        let cwd = context.shell.working_dir().to_path_buf();
        let archive = abs(&cwd, &self.archive);
        let f = std::fs::File::create(&archive)?;
        let mut zw = zip::ZipWriter::new(f);
        let opts: zip::write::FileOptions<()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let mut out = context.stdout();
        for p in &self.paths {
            let full = abs(&cwd, p);
            if full.is_dir() && self.recurse {
                for entry in walkdir::WalkDir::new(&full).into_iter().flatten() {
                    let rel = entry.path().strip_prefix(&cwd).unwrap_or(entry.path());
                    let name = rel.to_string_lossy().into_owned();
                    if entry.file_type().is_file() {
                        zw.start_file(&name, opts).map_err(std::io::Error::other)?;
                        let data = std::fs::read(entry.path())?;
                        zw.write_all(&data)?;
                        writeln!(out, "  adding: {name}")?;
                    }
                }
            } else if full.is_file() {
                zw.start_file(p, opts).map_err(std::io::Error::other)?;
                zw.write_all(&std::fs::read(&full)?)?;
                writeln!(out, "  adding: {p}")?;
            }
        }
        zw.finish().map_err(std::io::Error::other)?;
        out.flush()?;
        Ok(ExecutionResult::success())
    }
}

/// Extract a zip archive.
#[derive(Parser)]
pub struct UnzipCommand {
    archive: String,
    /// List contents only.
    #[arg(short = 'l')]
    list: bool,
    /// Extract into this directory.
    #[arg(short = 'd')]
    dir: Option<String>,
}

impl builtins::Command for UnzipCommand {
    type Error = brush_core::Error;
    async fn execute<SE: ShellExtensions>(
        &self,
        context: ExecutionContext<'_, SE>,
    ) -> Result<ExecutionResult, Self::Error> {
        let cwd = context.shell.working_dir().to_path_buf();
        let archive = abs(&cwd, &self.archive);
        let dest = self.dir.as_ref().map(|d| abs(&cwd, d)).unwrap_or(cwd);
        let f = std::fs::File::open(&archive)?;
        let mut zip = match zip::ZipArchive::new(f) {
            Ok(z) => z,
            Err(e) => {
                writeln!(context.stderr(), "unzip: {e}")?;
                return Ok(ExecutionResult::new(1));
            }
        };
        let mut out = context.stdout();
        if self.list {
            writeln!(out, "  Length      Name")?;
            for i in 0..zip.len() {
                let file = zip.by_index(i).map_err(std::io::Error::other)?;
                writeln!(out, "{:>9}  {}", file.size(), file.name())?;
            }
            out.flush()?;
            return Ok(ExecutionResult::success());
        }
        for i in 0..zip.len() {
            let mut file = zip.by_index(i).map_err(std::io::Error::other)?;
            let outpath = dest.join(file.mangled_name());
            if file.is_dir() {
                std::fs::create_dir_all(&outpath)?;
            } else {
                if let Some(parent) = outpath.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let mut w = std::fs::File::create(&outpath)?;
                std::io::copy(&mut file, &mut w)?;
                writeln!(out, " extracting: {}", file.name())?;
            }
        }
        out.flush()?;
        Ok(ExecutionResult::success())
    }
}

// ---------------------------------------------------------------- sqlite3

/// Minimal sqlite3 CLI: opens a database and runs SQL from an argument or
/// stdin, printing query results (pipe-separated, one row per line).
#[derive(Parser)]
pub struct SqliteCommand {
    /// Database file (`:memory:` for an in-memory db).
    database: Option<String>,
    /// SQL to execute; if omitted, read from stdin.
    sql: Option<String>,
    /// Print a header row of column names.
    #[arg(long = "header")]
    header: bool,
    /// Column separator (default: `|`).
    #[arg(long = "separator", default_value = "|")]
    separator: String,
}

impl builtins::Command for SqliteCommand {
    type Error = brush_core::Error;
    async fn execute<SE: ShellExtensions>(
        &self,
        context: ExecutionContext<'_, SE>,
    ) -> Result<ExecutionResult, Self::Error> {
        let cwd = context.shell.working_dir().to_path_buf();
        let db_path = self.database.clone().unwrap_or_else(|| ":memory:".to_string());
        let conn = if db_path == ":memory:" {
            rusqlite::Connection::open_in_memory()
        } else {
            rusqlite::Connection::open(abs(&cwd, &db_path))
        };
        let conn = match conn {
            Ok(c) => c,
            Err(e) => {
                writeln!(context.stderr(), "sqlite3: {e}")?;
                return Ok(ExecutionResult::new(1));
            }
        };

        let sql = if let Some(s) = &self.sql {
            s.clone()
        } else {
            let mut s = String::new();
            context.stdin().read_to_string(&mut s)?;
            s
        };
        if sql.trim().is_empty() {
            return Ok(ExecutionResult::success());
        }

        let mut out = context.stdout();
        let mut exit = 0u8;
        // Execute each statement; SELECTs print rows, others just run. Split on
        // `;` but not inside string literals (naive split(';') would break
        // `INSERT INTO t VALUES('a;b')`).
        for stmt_sql in split_sql_statements(&sql) {
            let trimmed = stmt_sql.trim();
            if trimmed.is_empty() {
                continue;
            }
            match run_sqlite_stmt(&conn, trimmed, self.header, &self.separator, &mut out) {
                Ok(()) => {}
                Err(e) => {
                    writeln!(context.stderr(), "sqlite3: {e}")?;
                    exit = 1;
                    break;
                }
            }
        }
        out.flush()?;
        Ok(ExecutionResult::new(exit))
    }
}

/// Splits a SQL script on `;`, but not inside single/double-quoted string
/// literals (where `''`/`""` are literal-quote escapes). Comments are not
/// specially handled — good enough for the common case a naive split breaks.
fn split_sql_statements(sql: &str) -> Vec<String> {
    let mut stmts = Vec::new();
    let mut cur = String::new();
    let mut chars = sql.chars().peekable();
    let mut in_single = false;
    let mut in_double = false;
    while let Some(c) = chars.next() {
        match c {
            '\'' if !in_double => {
                cur.push(c);
                if in_single {
                    if chars.peek() == Some(&'\'') {
                        cur.push(chars.next().unwrap()); // '' escape
                    } else {
                        in_single = false;
                    }
                } else {
                    in_single = true;
                }
            }
            '"' if !in_single => {
                cur.push(c);
                if in_double {
                    if chars.peek() == Some(&'"') {
                        cur.push(chars.next().unwrap()); // "" escape
                    } else {
                        in_double = false;
                    }
                } else {
                    in_double = true;
                }
            }
            ';' if !in_single && !in_double => {
                if !cur.trim().is_empty() {
                    stmts.push(std::mem::take(&mut cur));
                } else {
                    cur.clear();
                }
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        stmts.push(cur);
    }
    stmts
}

fn run_sqlite_stmt(
    conn: &rusqlite::Connection,
    sql: &str,
    header: bool,
    sep: &str,
    out: &mut impl Write,
) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(sql)?;
    let ncols = stmt.column_count();
    if ncols == 0 {
        // Non-query (INSERT/CREATE/…).
        conn.execute(sql, [])?;
        return Ok(());
    }
    let col_names: Vec<String> = stmt
        .column_names()
        .into_iter()
        .map(str::to_string)
        .collect();
    if header {
        let _ = writeln!(out, "{}", col_names.join(sep));
    }
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let mut cells = Vec::with_capacity(ncols);
        for i in 0..ncols {
            let v: rusqlite::types::Value = row.get(i)?;
            cells.push(match v {
                rusqlite::types::Value::Null => String::new(),
                rusqlite::types::Value::Integer(n) => n.to_string(),
                rusqlite::types::Value::Real(f) => f.to_string(),
                rusqlite::types::Value::Text(t) => t,
                rusqlite::types::Value::Blob(b) => format!("<{} bytes>", b.len()),
            });
        }
        let _ = writeln!(out, "{}", cells.join(sep));
    }
    Ok(())
}

// ---------------------------------------------------------------- jq

/// Process JSON with a jq filter, powered by jaq (pure-Rust jq).
#[derive(Parser)]
pub struct JqCommand {
    /// The jq filter program.
    filter: String,
    /// Input files (default: stdin).
    files: Vec<String>,
    /// Compact output (no pretty-printing — jaq is compact by default).
    #[arg(short = 'c')]
    _compact: bool,
    /// Raw output: strings without quotes.
    #[arg(short = 'r')]
    raw: bool,
    /// Read each line as a string instead of JSON.
    #[arg(short = 'R')]
    raw_input: bool,
    /// Don't read input; run the filter once with null input.
    #[arg(short = 'n')]
    null_input: bool,
}

impl builtins::Command for JqCommand {
    type Error = brush_core::Error;
    async fn execute<SE: ShellExtensions>(
        &self,
        context: ExecutionContext<'_, SE>,
    ) -> Result<ExecutionResult, Self::Error> {
        use jaq_core::load::{Arena, File, Loader};
        use jaq_core::{Compiler, Ctx, Vars};
        use jaq_json::Val;

        // Compile the filter.
        let program = File {
            code: self.filter.as_str(),
            path: (),
        };
        let defs = jaq_core::defs().chain(jaq_std::defs()).chain(jaq_json::defs());
        let funs = jaq_core::funs().chain(jaq_std::funs()).chain(jaq_json::funs());
        let loader = Loader::new(defs);
        let arena = Arena::default();
        let modules = match loader.load(&arena, program) {
            Ok(m) => m,
            Err(_) => {
                writeln!(context.stderr(), "jq: compile error in filter")?;
                return Ok(ExecutionResult::new(2));
            }
        };
        let filter = match Compiler::default().with_funs(funs).compile(modules) {
            Ok(f) => f,
            Err(_) => {
                writeln!(context.stderr(), "jq: compile error")?;
                return Ok(ExecutionResult::new(3));
            }
        };

        let cwd = context.shell.working_dir().to_path_buf();
        let mut out = context.stdout();

        // Gather input bytes.
        let raw = if self.null_input {
            Vec::new()
        } else if self.files.is_empty() {
            let mut b = Vec::new();
            context.stdin().read_to_end(&mut b)?;
            b
        } else {
            let mut b = Vec::new();
            for f in &self.files {
                b.extend(std::fs::read(abs(&cwd, f)).unwrap_or_default());
            }
            b
        };

        // Build the input value stream.
        let inputs: Vec<Val> = if self.null_input {
            vec![Val::Null]
        } else if self.raw_input {
            String::from_utf8_lossy(&raw)
                .lines()
                .map(|l| Val::from(l.to_string()))
                .collect()
        } else {
            jaq_json::read::parse_many(&raw).filter_map(Result::ok).collect()
        };

        let mut exit = 0u8;
        for input in inputs {
            let ctx = Ctx::<jaq_core::data::JustLut<Val>>::new(&filter.lut, Vars::new([]));
            for result in filter.id.run((ctx, input)) {
                match result {
                    Ok(v) => {
                        if self.raw {
                            match &v {
                                Val::TStr(b) | Val::BStr(b) => {
                                    writeln!(out, "{}", String::from_utf8_lossy(b.as_ref()))?;
                                    continue;
                                }
                                _ => {}
                            }
                        }
                        writeln!(out, "{v}")?;
                    }
                    Err(e) => {
                        writeln!(context.stderr(), "jq: error: {e:?}")?;
                        exit = 5;
                    }
                }
            }
        }
        out.flush()?;
        Ok(ExecutionResult::new(exit))
    }
}

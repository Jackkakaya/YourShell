//! `git` command backed by libgit2 (git2 crate, vendored). In-process, no
//! fork/exec. HTTPS transport (SecureTransport-equivalent via vendored
//! OpenSSL). Covers the common porcelain: init/clone/status/add/commit/log/
//! diff/branch/checkout/pull/push/config/remote. Auth for private repos via
//! GIT_USERNAME + GIT_PASSWORD (or token) env vars.

use std::io::Write;
use std::path::Path;

use brush_core::{builtins, ExecutionContext, ExecutionResult, ShellExtensions};
use git2::{Repository, RepositoryInitOptions, ResetType, Signature};

#[derive(Clone, Default)]
struct GitCredentials {
    username: Option<String>,
    password: Option<String>,
    token: Option<String>,
    ssh_key_path: Option<String>,
    ssh_public_key_path: Option<String>,
    ssh_private_key: Option<String>,
    ssh_public_key: Option<String>,
    ssh_passphrase: Option<String>,
    ssh_hostkey_sha256: Option<String>,
}

impl GitCredentials {
    fn from_shell<SE: ShellExtensions>(shell: &brush_core::Shell<SE>) -> Self {
        let value = |name: &str| {
            shell
                .env()
                .get_str(name, shell)
                .map(|value| value.into_owned())
        };
        Self {
            username: value("GIT_USERNAME"),
            password: value("GIT_PASSWORD"),
            token: value("GIT_TOKEN"),
            ssh_key_path: value("GIT_SSH_KEY"),
            ssh_public_key_path: value("GIT_SSH_PUBLIC_KEY"),
            ssh_private_key: value("GIT_SSH_PRIVATE_KEY"),
            ssh_public_key: value("GIT_SSH_PUBLIC_KEY_DATA"),
            ssh_passphrase: value("GIT_SSH_PASSPHRASE"),
            ssh_hostkey_sha256: value("GIT_SSH_HOSTKEY_SHA256"),
        }
    }
}

fn open_repo(
    cwd: &Path,
    git_dir: Option<&Path>,
    work_tree: Option<&Path>,
) -> Result<Repository, git2::Error> {
    let repo = match git_dir {
        Some(path) => Repository::open(path)?,
        None => Repository::discover(cwd)?,
    };
    if let Some(path) = work_tree {
        repo.set_workdir(path, false)?;
    }
    Ok(repo)
}

fn cred_callback(credentials: GitCredentials) -> git2::RemoteCallbacks<'static> {
    let mut cb = git2::RemoteCallbacks::new();
    let expected_hostkey = credentials.ssh_hostkey_sha256.clone();
    cb.certificate_check(move |certificate, host| {
        let Some(expected) = expected_hostkey.as_deref() else {
            return Ok(git2::CertificateCheckStatus::CertificatePassthrough);
        };
        let Some(hash) = certificate
            .as_hostkey()
            .and_then(|hostkey| hostkey.hash_sha256())
        else {
            return Ok(git2::CertificateCheckStatus::CertificatePassthrough);
        };
        let actual = hash
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let expected = expected
            .trim()
            .trim_start_matches("SHA256:")
            .replace(':', "")
            .to_ascii_lowercase();
        if actual == expected {
            Ok(git2::CertificateCheckStatus::CertificateOk)
        } else {
            Err(git2::Error::from_str(&format!(
                "SSH host key mismatch for {host}"
            )))
        }
    });
    cb.credentials(move |_url, username_from_url, allowed| {
        let username = credentials
            .username
            .clone()
            .or_else(|| username_from_url.map(str::to_string))
            .unwrap_or_else(|| "git".to_string());
        if allowed.contains(git2::CredentialType::SSH_KEY) {
            if let Some(private_key) = credentials.ssh_key_path.as_deref() {
                return git2::Cred::ssh_key(
                    &username,
                    credentials.ssh_public_key_path.as_deref().map(Path::new),
                    Path::new(private_key),
                    credentials.ssh_passphrase.as_deref(),
                );
            }
            if let Some(private_key) = credentials.ssh_private_key.as_deref() {
                return git2::Cred::ssh_key_from_memory(
                    &username,
                    credentials.ssh_public_key.as_deref(),
                    private_key,
                    credentials.ssh_passphrase.as_deref(),
                );
            }
        }
        if allowed.contains(git2::CredentialType::USER_PASS_PLAINTEXT) {
            if let (Some(user), Some(pass)) = (
                credentials.username.as_deref(),
                credentials.password.as_deref(),
            ) {
                return git2::Cred::userpass_plaintext(&user, &pass);
            }
            if let Some(token) = credentials.token.as_deref() {
                return git2::Cred::userpass_plaintext(&username, &token);
            }
        }
        git2::Cred::default()
    });
    cb
}

pub fn registration() -> builtins::Registration<brush_core::extensions::DefaultShellExtensions> {
    builtins::Registration {
        execute_func: exec,
        content_func: content,
        disabled: false,
        special_builtin: false,
        declaration_builtin: false,
    }
}

fn content(
    _name: &str,
    _t: builtins::ContentType,
    _o: &builtins::ContentOptions,
) -> Result<String, brush_core::Error> {
    Ok("git: version control (libgit2, in-process)".to_string())
}

fn exec(
    context: ExecutionContext<'_, brush_core::extensions::DefaultShellExtensions>,
    args: Vec<brush_core::CommandArg>,
) -> futures::future::BoxFuture<'_, Result<ExecutionResult, brush_core::Error>> {
    Box::pin(async move {
        let argv: Vec<String> = args.iter().skip(1).map(ToString::to_string).collect();
        let cwd = context.shell.working_dir().to_path_buf();
        let code = run_git(&argv, &cwd, &context);
        // libgit2 resets SIGPIPE to its default somewhere in the repository /
        // commit path, which makes the NEXT command writing into a reader-less
        // pipe kill the whole app. Found by bisecting the battery down to two
        // cases: `git init && git add && git commit` followed by any
        // early-exiting pipe reader is fatal, while either alone is harmless.
        // This adapter runs outside `command_host`, so re-assert here.
        crate::command_host::ensure_sigpipe_ignored();
        Ok(ExecutionResult::new(code))
    })
}

#[allow(clippy::too_many_lines)]
fn run_git<SE: ShellExtensions>(
    argv: &[String],
    cwd: &Path,
    context: &ExecutionContext<'_, SE>,
) -> u8 {
    let credentials = GitCredentials::from_shell(context.shell);
    let mut out = context.stdout();
    let mut err = context.stderr();
    // Git global options precede the subcommand. Implement the most important
    // one (`-C`) before dispatching so scripts can target another worktree
    // without changing the Brush session cwd.
    let mut global_end = 0;
    let mut effective_cwd = cwd.to_path_buf();
    let mut git_dir = None;
    let mut work_tree = None;
    while global_end < argv.len() {
        match argv[global_end].as_str() {
            "-C" => {
                global_end += 1;
                let Some(path) = argv.get(global_end) else {
                    let _ = writeln!(err, "git: -C requires a path");
                    return 2;
                };
                let next = Path::new(path);
                effective_cwd = if next.is_absolute() {
                    next.to_path_buf()
                } else {
                    effective_cwd.join(next)
                };
            }
            _ if argv[global_end].starts_with("-C") && argv[global_end].len() > 2 => {
                effective_cwd = effective_cwd.join(&argv[global_end][2..]);
            }
            "--git-dir" | "--work-tree" => {
                let option = argv[global_end].clone();
                global_end += 1;
                let Some(path) = argv.get(global_end) else {
                    let _ = writeln!(err, "git: {option} requires a path");
                    return 2;
                };
                let resolved = resolve_path(&effective_cwd, path);
                if option == "--git-dir" {
                    git_dir = Some(resolved);
                } else {
                    work_tree = Some(resolved);
                }
            }
            option if option.starts_with("--git-dir=") => {
                git_dir = Some(resolve_path(
                    &effective_cwd,
                    option.trim_start_matches("--git-dir="),
                ));
            }
            option if option.starts_with("--work-tree=") => {
                work_tree = Some(resolve_path(
                    &effective_cwd,
                    option.trim_start_matches("--work-tree="),
                ));
            }
            "--no-pager" | "-P" => {}
            _ => break,
        }
        global_end += 1;
    }
    let cwd = effective_cwd.as_path();
    let Some(sub) = argv.get(global_end) else {
        let _ = writeln!(err, "usage: git <command> [args]");
        return 1;
    };
    let rest = &argv[global_end + 1..];

    macro_rules! fail {
        ($e:expr) => {{
            let _ = writeln!(err, "git: {}", $e);
            return 1;
        }};
    }

    match sub.as_str() {
        "init" => {
            let (dir, quiet, bare, initial_branch) = match parse_init_args(rest, cwd) {
                Ok(parsed) => parsed,
                Err(message) => fail!(message),
            };
            let mut options = RepositoryInitOptions::new();
            options.bare(bare);
            if let Some(branch) = initial_branch.as_deref() {
                options.initial_head(branch);
            }
            match Repository::init_opts(&dir, &options) {
                Ok(_) => {
                    if !quiet {
                        let _ =
                            writeln!(out, "Initialized empty Git repository in {}", dir.display());
                    }
                    0
                }
                Err(e) => fail!(e),
            }
        }
        "clone" => git_clone(rest, cwd, credentials.clone(), &mut out, &mut err),
        "ls-remote" => git_ls_remote(rest, credentials.clone(), &mut out, &mut err),
        "status" => match open_repo(cwd, git_dir.as_deref(), work_tree.as_deref()) {
            Ok(repo) => git_status(&repo, rest, &mut out, &mut err),
            Err(e) => fail!(e),
        },
        "add" => match open_repo(cwd, git_dir.as_deref(), work_tree.as_deref()) {
            Ok(repo) => {
                let mut index = match repo.index() {
                    Ok(i) => i,
                    Err(e) => fail!(e),
                };
                let workdir = repo.workdir().map(Path::to_path_buf);
                for p in rest {
                    if p == "." || p == "-A" {
                        if let Err(e) =
                            index.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
                        {
                            fail!(e);
                        }
                    } else {
                        // A user path is relative to cwd, but add_path wants a
                        // repo-root-relative path — convert it (cwd may be a
                        // subdirectory of the repo).
                        let abs = cwd.join(p);
                        let rel = workdir
                            .as_deref()
                            .and_then(|wd| abs.strip_prefix(wd).ok())
                            .map_or_else(|| Path::new(p).to_path_buf(), Path::to_path_buf);
                        if let Err(e) = index.add_path(&rel) {
                            let _ = writeln!(err, "git: cannot add '{p}': {e}");
                            return 1;
                        }
                    }
                }
                if let Err(e) = index.write() {
                    fail!(e);
                }
                0
            }
            Err(e) => fail!(e),
        },
        "commit" => {
            let msg = parse_commit_message(rest).unwrap_or_default();
            let amend = rest.iter().any(|arg| arg == "--amend");
            let allow_empty = rest.iter().any(|arg| arg == "--allow-empty");
            if msg.is_empty() && !amend {
                fail!("commit requires -m <message>");
            }
            match open_repo(cwd, git_dir.as_deref(), work_tree.as_deref()) {
                Ok(repo) => {
                    if rest.iter().any(|arg| {
                        arg == "-a"
                            || arg == "--all"
                            || (arg.starts_with('-')
                                && !arg.starts_with("--")
                                && arg[1..].contains('a'))
                    }) {
                        let mut index = match repo.index() {
                            Ok(index) => index,
                            Err(error) => fail!(error),
                        };
                        let statuses = match repo.statuses(None) {
                            Ok(statuses) => statuses,
                            Err(error) => fail!(error),
                        };
                        for entry in statuses.iter() {
                            let status = entry.status();
                            let Ok(path) = entry.path() else { continue };
                            if status.is_wt_deleted() {
                                if let Err(error) = index.remove_path(Path::new(path)) {
                                    fail!(error);
                                }
                            } else if status.is_wt_modified()
                                || status.is_wt_typechange()
                                || status.is_wt_renamed()
                            {
                                if let Err(error) = index.add_path(Path::new(path)) {
                                    fail!(error);
                                }
                            }
                        }
                        if let Err(error) = index.write() {
                            fail!(error);
                        }
                    }
                    git_commit(&repo, &msg, amend, allow_empty, &mut out).map_or_else(
                        |error| {
                            let _ = writeln!(err, "git: {error}");
                            1
                        },
                        |()| 0,
                    )
                }
                Err(e) => fail!(e),
            }
        }
        "log" => match open_repo(cwd, git_dir.as_deref(), work_tree.as_deref()) {
            Ok(repo) => git_log(&repo, rest, &mut out, &mut err),
            Err(e) => fail!(e),
        },
        "diff" => match open_repo(cwd, git_dir.as_deref(), work_tree.as_deref()) {
            Ok(repo) => git_diff(&repo, rest, &mut out, &mut err),
            Err(e) => fail!(e),
        },
        "branch" => match open_repo(cwd, git_dir.as_deref(), work_tree.as_deref()) {
            Ok(repo) => git_branch(&repo, rest, &mut out, &mut err),
            Err(e) => fail!(e),
        },
        "checkout" => match open_repo(cwd, git_dir.as_deref(), work_tree.as_deref()) {
            Ok(repo) => {
                let Some(target) = rest.iter().find(|a| !a.starts_with('-')) else {
                    fail!("checkout requires a target");
                };
                git_checkout(&repo, target, rest.contains(&"-b".to_string()), &mut out)
                    .map_or(1, |()| 0)
            }
            Err(e) => fail!(e),
        },
        "switch" => match open_repo(cwd, git_dir.as_deref(), work_tree.as_deref()) {
            Ok(repo) => {
                let create = rest.iter().any(|arg| arg == "-c" || arg == "--create");
                let target = rest.iter().enumerate().find_map(|(index, arg)| {
                    if arg == "-c" || arg == "--create" {
                        rest.get(index + 1)
                    } else if !arg.starts_with('-')
                        && index
                            .checked_sub(1)
                            .and_then(|previous| rest.get(previous))
                            .is_none_or(|previous| previous != "-c" && previous != "--create")
                    {
                        Some(arg)
                    } else {
                        None
                    }
                });
                let Some(target) = target else {
                    fail!("switch requires a branch");
                };
                git_checkout(&repo, target, create, &mut out).map_or(1, |()| 0)
            }
            Err(e) => fail!(e),
        },
        "tag" => match open_repo(cwd, git_dir.as_deref(), work_tree.as_deref()) {
            Ok(repo) => git_tag(&repo, rest, &mut out, &mut err),
            Err(e) => fail!(e),
        },
        "show" => match open_repo(cwd, git_dir.as_deref(), work_tree.as_deref()) {
            Ok(repo) => {
                let spec = rest
                    .iter()
                    .find(|arg| !arg.starts_with('-'))
                    .map_or("HEAD", String::as_str);
                git_show(&repo, spec, &mut out, &mut err)
            }
            Err(e) => fail!(e),
        },
        "reset" => match open_repo(cwd, git_dir.as_deref(), work_tree.as_deref()) {
            Ok(repo) => git_reset(&repo, rest, &mut err),
            Err(e) => fail!(e),
        },
        "restore" => match open_repo(cwd, git_dir.as_deref(), work_tree.as_deref()) {
            Ok(repo) => git_restore(&repo, rest, cwd, &mut err),
            Err(e) => fail!(e),
        },
        "clean" => match open_repo(cwd, git_dir.as_deref(), work_tree.as_deref()) {
            Ok(repo) => git_clean(&repo, rest, &mut out, &mut err),
            Err(e) => fail!(e),
        },
        "rev-parse" => match open_repo(cwd, git_dir.as_deref(), work_tree.as_deref()) {
            Ok(repo) => git_rev_parse(&repo, rest, &mut out, &mut err),
            Err(e) => fail!(e),
        },
        "ls-files" => match open_repo(cwd, git_dir.as_deref(), work_tree.as_deref()) {
            Ok(repo) => git_ls_files(&repo, rest, &mut out, &mut err),
            Err(e) => fail!(e),
        },
        "ls-tree" => match open_repo(cwd, git_dir.as_deref(), work_tree.as_deref()) {
            Ok(repo) => git_ls_tree(&repo, rest, &mut out, &mut err),
            Err(e) => fail!(e),
        },
        "show-ref" => match open_repo(cwd, git_dir.as_deref(), work_tree.as_deref()) {
            Ok(repo) => git_show_ref(&repo, rest, &mut out, &mut err),
            Err(e) => fail!(e),
        },
        "symbolic-ref" => match open_repo(cwd, git_dir.as_deref(), work_tree.as_deref()) {
            Ok(repo) => git_symbolic_ref(&repo, rest, &mut out, &mut err),
            Err(e) => fail!(e),
        },
        "merge-base" => match open_repo(cwd, git_dir.as_deref(), work_tree.as_deref()) {
            Ok(repo) => git_merge_base(&repo, rest, &mut out, &mut err),
            Err(e) => fail!(e),
        },
        "merge" => match open_repo(cwd, git_dir.as_deref(), work_tree.as_deref()) {
            Ok(repo) => git_merge(&repo, rest, &mut out, &mut err),
            Err(e) => fail!(e),
        },
        "cherry-pick" => match open_repo(cwd, git_dir.as_deref(), work_tree.as_deref()) {
            Ok(repo) => git_replay_commit(&repo, rest, false, &mut out, &mut err),
            Err(e) => fail!(e),
        },
        "revert" => match open_repo(cwd, git_dir.as_deref(), work_tree.as_deref()) {
            Ok(repo) => git_replay_commit(&repo, rest, true, &mut out, &mut err),
            Err(e) => fail!(e),
        },
        "stash" => match open_repo(cwd, git_dir.as_deref(), work_tree.as_deref()) {
            Ok(mut repo) => git_stash(&mut repo, rest, &mut out, &mut err),
            Err(e) => fail!(e),
        },
        "rebase" => match open_repo(cwd, git_dir.as_deref(), work_tree.as_deref()) {
            Ok(mut repo) => git_rebase(&mut repo, rest, &mut out, &mut err),
            Err(e) => fail!(e),
        },
        "rm" => match open_repo(cwd, git_dir.as_deref(), work_tree.as_deref()) {
            Ok(repo) => git_rm(&repo, rest, cwd, &mut err),
            Err(e) => fail!(e),
        },
        "mv" => match open_repo(cwd, git_dir.as_deref(), work_tree.as_deref()) {
            Ok(repo) => git_mv(&repo, rest, cwd, &mut err),
            Err(e) => fail!(e),
        },
        "pull" | "fetch" => match open_repo(cwd, git_dir.as_deref(), work_tree.as_deref()) {
            Ok(mut repo) => git_fetch(
                &mut repo,
                rest,
                sub == "pull",
                credentials.clone(),
                &mut out,
                &mut err,
            ),
            Err(e) => fail!(e),
        },
        "push" => match open_repo(cwd, git_dir.as_deref(), work_tree.as_deref()) {
            Ok(repo) => git_push(&repo, rest, credentials.clone(), &mut out, &mut err),
            Err(e) => fail!(e),
        },
        "config" => {
            match open_repo(cwd, git_dir.as_deref(), work_tree.as_deref())
                .and_then(|r| r.config())
                .or_else(|_| git2::Config::open_default())
            {
                Ok(cfg) => git_config(cfg, rest, &mut out, &mut err),
                Err(e) => fail!(e),
            }
        }
        "remote" => match open_repo(cwd, git_dir.as_deref(), work_tree.as_deref()) {
            Ok(repo) => git_remote(&repo, rest, &mut out, &mut err),
            Err(e) => fail!(e),
        },
        "--version" | "version" => {
            let v = git2::Version::get();
            let _ = writeln!(out, "git (libgit2 {})", v.libgit2_version().0);
            0
        }
        other => {
            let _ = writeln!(err, "git: '{other}' is not supported yet");
            1
        }
    }
}

fn url_repo_name(url: &str) -> String {
    url.trim_end_matches('/')
        .trim_end_matches(".git")
        .rsplit('/')
        .next()
        .unwrap_or("repo")
        .to_string()
}

fn git_ls_remote(
    args: &[String],
    credentials: GitCredentials,
    out: &mut impl Write,
    err: &mut impl Write,
) -> u8 {
    let heads_only = args.iter().any(|arg| arg == "--heads" || arg == "-h");
    let tags_only = args.iter().any(|arg| arg == "--tags" || arg == "-t");
    let refs_only = args.iter().any(|arg| arg == "--refs");
    let exit_code = args.iter().any(|arg| arg == "--exit-code");
    let positionals: Vec<&str> = args
        .iter()
        .filter(|arg| !arg.starts_with('-'))
        .map(String::as_str)
        .collect();
    let Some(url) = positionals.first() else {
        let _ = writeln!(
            err,
            "usage: git ls-remote [--heads] [--tags] <repository> [<refs>...]"
        );
        return 129;
    };
    let patterns = &positionals[1..];
    let mut remote = match git2::Remote::create_detached(*url) {
        Ok(remote) => remote,
        Err(error) => {
            let _ = writeln!(err, "fatal: {error}");
            return 128;
        }
    };
    let connection = match remote.connect_auth(
        git2::Direction::Fetch,
        Some(cred_callback(credentials)),
        None,
    ) {
        Ok(connection) => connection,
        Err(error) => {
            let _ = writeln!(err, "fatal: unable to access '{url}': {error}");
            return 128;
        }
    };
    let heads = match connection.list() {
        Ok(heads) => heads,
        Err(error) => {
            let _ = writeln!(err, "fatal: {error}");
            return 128;
        }
    };
    let mut found = false;
    for head in heads {
        let name = head.name();
        if (heads_only && !name.starts_with("refs/heads/"))
            || (tags_only && !name.starts_with("refs/tags/"))
            || (refs_only && name == "HEAD")
            || (!patterns.is_empty()
                && !patterns.iter().any(|pattern| {
                    name == *pattern
                        || name
                            .strip_prefix("refs/heads/")
                            .is_some_and(|short| short == *pattern)
                        || name
                            .strip_prefix("refs/tags/")
                            .is_some_and(|short| short == *pattern)
                }))
        {
            continue;
        }
        found = true;
        let _ = writeln!(out, "{}\t{name}", head.oid());
    }
    if exit_code && !found {
        2
    } else {
        0
    }
}

fn git_restore(repo: &Repository, args: &[String], cwd: &Path, err: &mut impl Write) -> u8 {
    let mut staged = false;
    let mut worktree = false;
    let mut source: Option<&str> = None;
    let mut paths = Vec::new();
    let mut after_separator = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--" => after_separator = true,
            "-S" | "--staged" if !after_separator => staged = true,
            "-W" | "--worktree" if !after_separator => worktree = true,
            "-s" | "--source" if !after_separator => {
                index += 1;
                let Some(value) = args.get(index) else {
                    let _ = writeln!(err, "git: restore --source requires a value");
                    return 2;
                };
                source = Some(value);
            }
            option if !after_separator && option.starts_with("--source=") => {
                source = Some(option.trim_start_matches("--source="));
            }
            option if !after_separator && option.starts_with('-') => {
                let _ = writeln!(err, "git: restore: unknown option '{option}'");
                return 129;
            }
            path => paths.push(path),
        }
        index += 1;
    }
    if paths.is_empty() {
        let _ = writeln!(err, "fatal: you must specify path(s) to restore");
        return 128;
    }
    if !staged && !worktree {
        worktree = true;
    }
    let workdir = repo.workdir().unwrap_or(cwd);
    let relative: Vec<_> = paths
        .iter()
        .map(|path| {
            let absolute = cwd.join(path);
            absolute
                .strip_prefix(workdir)
                .map_or_else(|_| Path::new(path).to_path_buf(), Path::to_path_buf)
        })
        .collect();

    if staged {
        let source = source.unwrap_or("HEAD");
        let object = match repo.revparse_single(source) {
            Ok(object) => object,
            Err(error) => {
                let _ = writeln!(err, "fatal: could not resolve {source}: {error}");
                return 128;
            }
        };
        if let Err(error) = repo.reset_default(Some(&object), relative.iter()) {
            let _ = writeln!(err, "fatal: {error}");
            return 128;
        }
    }

    if worktree {
        let mut checkout = git2::build::CheckoutBuilder::new();
        checkout.force();
        for path in &relative {
            checkout.path(path);
        }
        let result = if let Some(source) = source {
            repo.revparse_single(source)
                .and_then(|object| repo.checkout_tree(&object, Some(&mut checkout)))
        } else {
            repo.checkout_index(None, Some(&mut checkout))
        };
        if let Err(error) = result {
            let _ = writeln!(err, "fatal: {error}");
            return 128;
        }
    }
    0
}

fn git_clean(repo: &Repository, args: &[String], out: &mut impl Write, err: &mut impl Write) -> u8 {
    let short = |flag: char| {
        args.iter().any(|arg| {
            arg == &format!("-{flag}")
                || (arg.starts_with('-')
                    && !arg.starts_with("--")
                    && arg[1..].chars().any(|value| value == flag))
        })
    };
    let force = short('f') || args.iter().any(|arg| arg == "--force");
    let dry_run = short('n') || args.iter().any(|arg| arg == "--dry-run");
    let directories = short('d');
    let ignored_too = short('x');
    if !force && !dry_run {
        let _ = writeln!(
            err,
            "fatal: clean.requireForce defaults to true and neither -i, -n, nor -f given"
        );
        return 128;
    }
    if args.iter().any(|arg| arg == "-i" || arg == "--interactive") {
        let _ = writeln!(err, "fatal: interactive clean is not supported");
        return 128;
    }
    let mut options = git2::StatusOptions::new();
    options
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(ignored_too);
    let statuses = match repo.statuses(Some(&mut options)) {
        Ok(statuses) => statuses,
        Err(error) => {
            let _ = writeln!(err, "fatal: {error}");
            return 128;
        }
    };
    let Some(workdir) = repo.workdir() else {
        let _ = writeln!(err, "fatal: this operation must be run in a work tree");
        return 128;
    };
    let mut candidates: Vec<_> = statuses
        .iter()
        .filter(|entry| {
            entry.status().contains(git2::Status::WT_NEW)
                || (ignored_too && entry.status().contains(git2::Status::IGNORED))
        })
        .filter_map(|entry| entry.path().ok().map(str::to_string))
        .collect();
    candidates.sort_by_key(|path| std::cmp::Reverse(path.matches('/').count()));
    candidates.dedup();
    for path in candidates {
        let absolute = workdir.join(&path);
        if absolute.is_dir() && !directories {
            continue;
        }
        let action = if dry_run { "Would remove" } else { "Removing" };
        let _ = writeln!(out, "{action} {path}");
        if !dry_run {
            let result = if absolute.is_dir() {
                std::fs::remove_dir_all(&absolute)
            } else {
                std::fs::remove_file(&absolute)
            };
            if let Err(error) = result {
                let _ = writeln!(err, "warning: failed to remove {path}: {error}");
                return 1;
            }
            if directories {
                let mut parent = absolute.parent();
                while let Some(directory) = parent {
                    if directory == workdir || std::fs::remove_dir(directory).is_err() {
                        break;
                    }
                    parent = directory.parent();
                }
            }
        }
    }
    0
}

fn git_rev_parse(
    repo: &Repository,
    args: &[String],
    out: &mut impl Write,
    err: &mut impl Write,
) -> u8 {
    if args.is_empty() {
        let _ = writeln!(err, "git: rev-parse requires an argument");
        return 1;
    }
    for arg in args {
        let value = match arg.as_str() {
            "--git-dir" => repo.path().to_string_lossy().into_owned(),
            "--show-toplevel" => match repo.workdir() {
                Some(path) => path.to_string_lossy().trim_end_matches('/').to_string(),
                None => {
                    let _ = writeln!(err, "fatal: this operation must be run in a work tree");
                    return 128;
                }
            },
            "--is-bare-repository" => repo.is_bare().to_string(),
            "--is-inside-work-tree" => (!repo.is_bare()).to_string(),
            "--abbrev-ref" | "--verify" | "--quiet" => continue,
            spec => match repo.revparse_single(spec) {
                Ok(object) => object.id().to_string(),
                Err(error) => {
                    let _ = writeln!(err, "fatal: ambiguous argument '{spec}': {error}");
                    return 128;
                }
            },
        };
        let _ = writeln!(out, "{value}");
    }
    0
}

fn git_ls_files(
    repo: &Repository,
    args: &[String],
    out: &mut impl Write,
    err: &mut impl Write,
) -> u8 {
    let stage = args.iter().any(|arg| arg == "-s" || arg == "--stage");
    let nul = args.iter().any(|arg| arg == "-z");
    let index = match repo.index() {
        Ok(index) => index,
        Err(error) => {
            let _ = writeln!(err, "git: {error}");
            return 1;
        }
    };
    for entry in index.iter() {
        let path = String::from_utf8_lossy(&entry.path);
        if stage {
            let stage_number = (entry.flags >> 12) & 3;
            let _ = write!(
                out,
                "{:06o} {} {}\t{}",
                entry.mode, entry.id, stage_number, path
            );
        } else {
            let _ = write!(out, "{path}");
        }
        let _ = if nul {
            out.write_all(&[0])
        } else {
            writeln!(out)
        };
    }
    0
}

fn git_ls_tree(
    repo: &Repository,
    args: &[String],
    out: &mut impl Write,
    err: &mut impl Write,
) -> u8 {
    let recursive = args.iter().any(|arg| arg == "-r");
    let names_only = args
        .iter()
        .any(|arg| arg == "--name-only" || arg == "--name-status");
    let nul = args.iter().any(|arg| arg == "-z");
    let spec = args
        .iter()
        .find(|arg| !arg.starts_with('-'))
        .map_or("HEAD", String::as_str);
    let tree = match repo
        .revparse_single(spec)
        .and_then(|object| object.peel_to_tree())
    {
        Ok(tree) => tree,
        Err(error) => {
            let _ = writeln!(err, "fatal: not a tree object: {error}");
            return 128;
        }
    };
    let mode = if recursive {
        git2::TreeWalkMode::PreOrder
    } else {
        git2::TreeWalkMode::PreOrder
    };
    let result = tree.walk(mode, |root, entry| {
        if !recursive && !root.is_empty() {
            return git2::TreeWalkResult::Skip;
        }
        let name = entry.name().unwrap_or("");
        let path = format!("{root}{name}");
        if names_only {
            let _ = write!(out, "{path}");
        } else {
            let kind = match entry.kind() {
                Some(git2::ObjectType::Tree) => "tree",
                Some(git2::ObjectType::Commit) => "commit",
                _ => "blob",
            };
            let _ = write!(
                out,
                "{:06o} {kind} {}\t{path}",
                entry.filemode(),
                entry.id()
            );
        }
        let _ = if nul {
            out.write_all(&[0])
        } else {
            writeln!(out)
        };
        if recursive {
            git2::TreeWalkResult::Ok
        } else {
            git2::TreeWalkResult::Skip
        }
    });
    match result {
        Ok(()) => 0,
        Err(error) => {
            let _ = writeln!(err, "git: {error}");
            1
        }
    }
}

fn git_show_ref(
    repo: &Repository,
    args: &[String],
    out: &mut impl Write,
    err: &mut impl Write,
) -> u8 {
    let heads = args.iter().any(|arg| arg == "--heads");
    let tags = args.iter().any(|arg| arg == "--tags");
    let verify = args.iter().any(|arg| arg == "--verify");
    let quiet = args.iter().any(|arg| arg == "-q" || arg == "--quiet");
    let patterns: Vec<&str> = args
        .iter()
        .filter(|arg| !arg.starts_with('-'))
        .map(String::as_str)
        .collect();
    let refs = match repo.references() {
        Ok(refs) => refs,
        Err(error) => {
            let _ = writeln!(err, "git: {error}");
            return 1;
        }
    };
    let mut found = false;
    for reference in refs.flatten() {
        let Ok(name) = reference.name() else { continue };
        if (heads && !name.starts_with("refs/heads/"))
            || (tags && !name.starts_with("refs/tags/"))
            || (!patterns.is_empty()
                && !patterns.iter().any(|pattern| {
                    if verify {
                        name == *pattern
                    } else {
                        name.ends_with(pattern)
                    }
                }))
        {
            continue;
        }
        let Ok(resolved) = reference.resolve() else {
            continue;
        };
        let Some(oid) = resolved.target() else {
            continue;
        };
        found = true;
        if !quiet {
            let _ = writeln!(out, "{oid} {name}");
        }
    }
    u8::from(!found)
}

fn git_symbolic_ref(
    repo: &Repository,
    args: &[String],
    out: &mut impl Write,
    err: &mut impl Write,
) -> u8 {
    let quiet = args.iter().any(|arg| arg == "-q" || arg == "--quiet");
    let delete = args.iter().any(|arg| arg == "-d" || arg == "--delete");
    let values: Vec<&str> = args
        .iter()
        .filter(|arg| !arg.starts_with('-'))
        .map(String::as_str)
        .collect();
    let Some(name) = values.first() else {
        let _ = writeln!(err, "usage: git symbolic-ref [-q] [-d] <name> [<ref>]");
        return 2;
    };
    if delete {
        return match repo
            .find_reference(name)
            .and_then(|mut reference| reference.delete())
        {
            Ok(()) => 0,
            Err(error) => {
                if !quiet {
                    let _ = writeln!(err, "fatal: {error}");
                }
                1
            }
        };
    }
    if let Some(target) = values.get(1) {
        return match repo.reference_symbolic(name, target, true, "symbolic-ref") {
            Ok(_) => 0,
            Err(error) => {
                let _ = writeln!(err, "fatal: {error}");
                1
            }
        };
    }
    match repo.find_reference(name).ok().and_then(|reference| {
        reference
            .symbolic_target()
            .ok()
            .flatten()
            .map(str::to_string)
    }) {
        Some(target) => {
            let _ = writeln!(out, "{target}");
            0
        }
        None => {
            if !quiet {
                let _ = writeln!(err, "fatal: ref {name} is not a symbolic ref");
            }
            1
        }
    }
}

fn git_merge_base(
    repo: &Repository,
    args: &[String],
    out: &mut impl Write,
    err: &mut impl Write,
) -> u8 {
    let specs: Vec<&str> = args
        .iter()
        .filter(|arg| !arg.starts_with('-'))
        .map(String::as_str)
        .collect();
    if specs.len() != 2 {
        let _ = writeln!(err, "usage: git merge-base <commit> <commit>");
        return 2;
    }
    let ids = specs
        .iter()
        .map(|spec| repo.revparse_single(spec).map(|object| object.id()))
        .collect::<Result<Vec<_>, _>>();
    match ids.and_then(|ids| repo.merge_base(ids[0], ids[1])) {
        Ok(id) => {
            let _ = writeln!(out, "{id}");
            0
        }
        Err(error) => {
            let _ = writeln!(err, "fatal: {error}");
            1
        }
    }
}

fn git_clone(
    args: &[String],
    cwd: &Path,
    credentials: GitCredentials,
    out: &mut impl Write,
    err: &mut impl Write,
) -> u8 {
    let mut quiet = false;
    let mut bare = false;
    let mut branch = None;
    let mut depth = None;
    let mut origin = "origin".to_string();
    let mut positionals = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "-q" | "--quiet" => quiet = true,
            "--bare" => bare = true,
            "-b" | "--branch" | "--depth" | "-o" | "--origin" => {
                let option = args[index].clone();
                index += 1;
                let Some(value) = args.get(index) else {
                    let _ = writeln!(err, "git: clone {option} requires a value");
                    return 2;
                };
                match option.as_str() {
                    "-b" | "--branch" => branch = Some(value.clone()),
                    "--depth" => match value.parse::<i32>() {
                        Ok(value) if value > 0 => depth = Some(value),
                        _ => {
                            let _ = writeln!(err, "git: clone depth must be a positive integer");
                            return 2;
                        }
                    },
                    _ => origin = value.clone(),
                }
            }
            option if option.starts_with("--branch=") => {
                branch = Some(option.trim_start_matches("--branch=").to_string());
            }
            option if option.starts_with("--depth=") => {
                match option.trim_start_matches("--depth=").parse::<i32>() {
                    Ok(value) if value > 0 => depth = Some(value),
                    _ => {
                        let _ = writeln!(err, "git: clone depth must be a positive integer");
                        return 2;
                    }
                }
            }
            option if option.starts_with("--origin=") => {
                origin = option.trim_start_matches("--origin=").to_string();
            }
            "--single-branch" => {}
            "--" => {
                positionals.extend(args[index + 1..].iter().cloned());
                break;
            }
            option if option.starts_with('-') => {
                let _ = writeln!(err, "git: clone: unsupported option '{option}'");
                return 2;
            }
            value => positionals.push(value.to_string()),
        }
        index += 1;
    }
    let Some(url) = positionals.first() else {
        let _ = writeln!(err, "git: clone requires a URL");
        return 2;
    };
    if positionals.len() > 2 {
        let _ = writeln!(err, "git: clone accepts only one destination");
        return 2;
    }
    let directory = positionals.get(1).map_or_else(
        || cwd.join(url_repo_name(url)),
        |path| resolve_path(cwd, path),
    );
    let mut fetch_options = git2::FetchOptions::new();
    fetch_options.remote_callbacks(cred_callback(credentials));
    if let Some(depth) = depth {
        fetch_options.depth(depth);
    }
    let mut builder = git2::build::RepoBuilder::new();
    builder.bare(bare).fetch_options(fetch_options);
    if let Some(branch) = branch.as_deref() {
        builder.branch(branch);
    }
    if origin != "origin" {
        builder.remote_create(move |repo, _name, url| repo.remote(&origin, url));
    }
    match builder.clone(url, &directory) {
        Ok(_) => {
            if !quiet {
                let _ = writeln!(out, "Cloned into '{}'", directory.display());
            }
            0
        }
        Err(error) => {
            let _ = writeln!(err, "git: clone failed: {error}");
            1
        }
    }
}

fn resolve_path(cwd: &Path, path: &str) -> std::path::PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

fn parse_init_args(
    args: &[String],
    cwd: &Path,
) -> Result<(std::path::PathBuf, bool, bool, Option<String>), String> {
    let mut directory = None;
    let mut quiet = false;
    let mut bare = false;
    let mut initial_branch = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "-q" | "--quiet" => quiet = true,
            "--bare" => bare = true,
            "-b" | "--initial-branch" => {
                index += 1;
                initial_branch = Some(
                    args.get(index)
                        .cloned()
                        .ok_or_else(|| format!("{} requires a branch name", args[index - 1]))?,
                );
            }
            option if option.starts_with("--initial-branch=") => {
                initial_branch = Some(option.trim_start_matches("--initial-branch=").to_string());
            }
            "--" => {
                index += 1;
                if let Some(path) = args.get(index) {
                    directory = Some(resolve_path(cwd, path));
                }
                if index + 1 < args.len() {
                    return Err("init accepts at most one directory".to_string());
                }
                break;
            }
            option if option.starts_with('-') => {
                return Err(format!("init: unsupported option '{option}'"));
            }
            path => {
                if directory.is_some() {
                    return Err("init accepts at most one directory".to_string());
                }
                directory = Some(resolve_path(cwd, path));
            }
        }
        index += 1;
    }
    Ok((
        directory.unwrap_or_else(|| cwd.to_path_buf()),
        quiet,
        bare,
        initial_branch,
    ))
}

fn parse_flag(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}

fn parse_commit_message(args: &[String]) -> Option<String> {
    for (index, arg) in args.iter().enumerate() {
        if arg == "-m" || arg == "--message" || arg.ends_with('m') && arg.starts_with('-') {
            return args.get(index + 1).cloned();
        }
        if let Some(message) = arg.strip_prefix("--message=") {
            return Some(message.to_string());
        }
        if let Some(message) = arg.strip_prefix("-m").filter(|message| !message.is_empty()) {
            return Some(message.to_string());
        }
    }
    None
}

fn signature(repo: &Repository) -> Result<Signature<'static>, git2::Error> {
    let cfg = repo.config()?;
    let name = cfg
        .get_string("user.name")
        .or_else(|_| std::env::var("GIT_AUTHOR_NAME").map_err(|_| git2::Error::from_str("no name")))
        .unwrap_or_else(|_| "YourShell User".to_string());
    let email = cfg
        .get_string("user.email")
        .or_else(|_| {
            std::env::var("GIT_AUTHOR_EMAIL").map_err(|_| git2::Error::from_str("no email"))
        })
        .unwrap_or_else(|_| "user@yourshell.local".to_string());
    Signature::now(&name, &email)
}

fn git_status(
    repo: &Repository,
    args: &[String],
    out: &mut impl Write,
    err: &mut impl Write,
) -> u8 {
    let porcelain = args.iter().any(|arg| {
        arg == "-s" || arg == "--short" || arg == "--porcelain" || arg.starts_with("--porcelain=")
    });
    if args
        .iter()
        .any(|arg| arg.starts_with("--porcelain=") && arg != "--porcelain=v1")
    {
        let _ = writeln!(err, "git: only porcelain v1 is currently supported");
        return 2;
    }
    let show_branch = args.iter().any(|arg| arg == "-b" || arg == "--branch");
    let head = current_branch(repo);
    let statuses = match repo.statuses(None) {
        Ok(statuses) => statuses,
        Err(error) => {
            let _ = writeln!(err, "git: {error}");
            return 1;
        }
    };
    if porcelain {
        if show_branch {
            let _ = writeln!(out, "## {}", head.as_deref().unwrap_or("HEAD (no branch)"));
        }
        for entry in statuses.iter() {
            let status = entry.status();
            let path = entry.path().unwrap_or("?");
            if status.is_conflicted() {
                let _ = writeln!(out, "UU {path}");
                continue;
            }
            if status.is_wt_new() && !status.is_index_new() {
                let _ = writeln!(out, "?? {path}");
                continue;
            }
            if status.is_ignored() {
                let _ = writeln!(out, "!! {path}");
                continue;
            }
            let index = if status.is_index_new() {
                'A'
            } else if status.is_index_modified() {
                'M'
            } else if status.is_index_deleted() {
                'D'
            } else if status.is_index_renamed() {
                'R'
            } else if status.is_index_typechange() {
                'T'
            } else {
                ' '
            };
            let worktree = if status.is_wt_modified() {
                'M'
            } else if status.is_wt_deleted() {
                'D'
            } else if status.is_wt_renamed() {
                'R'
            } else if status.is_wt_typechange() {
                'T'
            } else {
                ' '
            };
            let _ = writeln!(out, "{index}{worktree} {path}");
        }
        return 0;
    }
    let _ = writeln!(
        out,
        "On branch {}",
        head.as_deref().unwrap_or("(no branch)")
    );
    if statuses.is_empty() {
        let _ = writeln!(out, "nothing to commit, working tree clean");
        return 0;
    }
    for entry in statuses.iter() {
        let s = entry.status();
        let tag = if s.is_wt_new() || s.is_index_new() {
            "new:"
        } else if s.is_wt_modified() || s.is_index_modified() {
            "modified:"
        } else if s.is_wt_deleted() || s.is_index_deleted() {
            "deleted:"
        } else {
            "changed:"
        };
        let _ = writeln!(out, "  {tag} {}", entry.path().unwrap_or("?"));
    }
    0
}

fn current_branch(repo: &Repository) -> Option<String> {
    repo.head()
        .ok()
        .and_then(|head| head.shorthand().ok().map(String::from))
        .or_else(|| {
            repo.find_reference("HEAD")
                .ok()
                .and_then(|head| head.symbolic_target().ok().flatten().map(String::from))
                .and_then(|name| name.strip_prefix("refs/heads/").map(String::from))
        })
}

fn git_commit(
    repo: &Repository,
    msg: &str,
    amend: bool,
    allow_empty: bool,
    out: &mut impl Write,
) -> Result<(), git2::Error> {
    let sig = signature(repo)?;
    let mut index = repo.index()?;
    let tree_id = index.write_tree()?;
    let tree = repo.find_tree(tree_id)?;
    if amend {
        let head = repo.head()?.peel_to_commit()?;
        let message = if msg.is_empty() {
            head.message().unwrap_or("")
        } else {
            msg
        };
        let oid = head.amend(
            Some("HEAD"),
            Some(&sig),
            Some(&sig),
            None,
            Some(message),
            Some(&tree),
        )?;
        let _ = writeln!(
            out,
            "[{}] {}",
            &oid.to_string()[..7],
            message.lines().next().unwrap_or("")
        );
        return Ok(());
    }
    let parents = repo
        .head()
        .ok()
        .and_then(|h| h.peel_to_commit().ok())
        .into_iter()
        .collect::<Vec<_>>();
    if !allow_empty
        && parents
            .first()
            .is_some_and(|parent| parent.tree_id() == tree_id)
    {
        return Err(git2::Error::from_str(
            "nothing to commit, working tree clean",
        ));
    }
    let parent_refs: Vec<&git2::Commit> = parents.iter().collect();
    let oid = repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &parent_refs)?;
    let _ = writeln!(
        out,
        "[{}] {}",
        &oid.to_string()[..7],
        msg.lines().next().unwrap_or("")
    );
    Ok(())
}

fn git_branch(
    repo: &Repository,
    args: &[String],
    out: &mut impl Write,
    err: &mut impl Write,
) -> u8 {
    let positionals: Vec<&str> = args
        .iter()
        .filter(|arg| !arg.starts_with('-'))
        .map(String::as_str)
        .collect();
    let delete = args.iter().any(|arg| arg == "-d" || arg == "--delete");
    let force_delete = args.iter().any(|arg| arg == "-D");
    if delete || force_delete {
        if positionals.is_empty() {
            let _ = writeln!(err, "git: branch delete requires a branch name");
            return 2;
        }
        let head_id = repo.head().ok().and_then(|head| head.target());
        for name in positionals {
            let mut branch = match repo.find_branch(name, git2::BranchType::Local) {
                Ok(branch) => branch,
                Err(error) => {
                    let _ = writeln!(err, "error: branch '{name}' not found: {error}");
                    return 1;
                }
            };
            let target = branch.get().target();
            let unmerged = if let (Some(head), Some(target)) = (head_id, target) {
                head != target && !repo.graph_descendant_of(head, target).unwrap_or(false)
            } else {
                false
            };
            if !force_delete && unmerged {
                let _ = writeln!(
                    err,
                    "error: the branch '{name}' is not fully merged; use -D to force deletion"
                );
                return 1;
            }
            if let Err(error) = branch.delete() {
                let _ = writeln!(err, "error: cannot delete branch '{name}': {error}");
                return 1;
            }
            let _ = writeln!(out, "Deleted branch {name}");
        }
        return 0;
    }
    if args.iter().any(|arg| arg == "-m" || arg == "--move") {
        let (old_name, new_name) = if positionals.len() == 1 {
            let Some(current) = current_branch(repo) else {
                let _ = writeln!(err, "fatal: cannot rename a detached HEAD");
                return 128;
            };
            (current, positionals[0].to_string())
        } else if positionals.len() == 2 {
            (positionals[0].to_string(), positionals[1].to_string())
        } else {
            let _ = writeln!(err, "usage: git branch -m [<oldbranch>] <newbranch>");
            return 2;
        };
        let mut branch = match repo.find_branch(&old_name, git2::BranchType::Local) {
            Ok(branch) => branch,
            Err(error) => {
                let _ = writeln!(err, "fatal: {error}");
                return 128;
            }
        };
        return match branch.rename(&new_name, false) {
            Ok(_) => 0,
            Err(error) => {
                let _ = writeln!(err, "fatal: {error}");
                128
            }
        };
    }
    let remote_only = args.iter().any(|arg| arg == "-r" || arg == "--remotes");
    let all = args.iter().any(|arg| arg == "-a" || arg == "--all");
    if positionals.is_empty() {
        let kinds: &[git2::BranchType] = if all {
            &[git2::BranchType::Local, git2::BranchType::Remote]
        } else if remote_only {
            &[git2::BranchType::Remote]
        } else {
            &[git2::BranchType::Local]
        };
        let current = current_branch(repo);
        for kind in kinds {
            let Ok(branches) = repo.branches(Some(*kind)) else {
                continue;
            };
            for branch in branches.flatten() {
                if let Ok(Some(name)) = branch.0.name() {
                    let marker =
                        if *kind == git2::BranchType::Local && Some(name) == current.as_deref() {
                            "* "
                        } else {
                            "  "
                        };
                    let _ = writeln!(out, "{marker}{name}");
                }
            }
        }
        return 0;
    }
    if positionals.len() > 2 {
        let _ = writeln!(err, "usage: git branch <branchname> [<start-point>]");
        return 2;
    }
    let name = positionals[0];
    let start = positionals.get(1).copied().unwrap_or("HEAD");
    let commit = match repo
        .revparse_single(start)
        .and_then(|object| object.peel_to_commit())
    {
        Ok(commit) => commit,
        Err(error) => {
            let _ = writeln!(err, "fatal: not a valid object name: {error}");
            return 128;
        }
    };
    match repo.branch(name, &commit, false) {
        Ok(_) => 0,
        Err(error) => {
            let _ = writeln!(err, "fatal: {error}");
            128
        }
    }
}

fn git_log(repo: &Repository, args: &[String], out: &mut impl Write, err: &mut impl Write) -> u8 {
    let mut n = 20;
    let mut oneline = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--oneline" => oneline = true,
            "-n" | "--max-count" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    let _ = writeln!(err, "git: log {} requires a value", args[index - 1]);
                    return 2;
                };
                let Ok(value) = value.parse() else {
                    let _ = writeln!(err, "git: log invalid max count '{value}'");
                    return 2;
                };
                n = value;
            }
            option if option.starts_with("--max-count=") => {
                let Ok(value) = option.trim_start_matches("--max-count=").parse() else {
                    let _ = writeln!(err, "git: log invalid max count");
                    return 2;
                };
                n = value;
            }
            option
                if option.starts_with('-')
                    && option.len() > 1
                    && option[1..]
                        .chars()
                        .all(|character| character.is_ascii_digit()) =>
            {
                n = option[1..].parse().unwrap_or(20);
            }
            "--no-decorate" => {}
            option if option.starts_with('-') => {
                let _ = writeln!(err, "git: log unsupported option '{option}'");
                return 2;
            }
            _ => {}
        }
        index += 1;
    }
    let mut revwalk = match repo.revwalk() {
        Ok(revwalk) => revwalk,
        Err(error) => {
            let _ = writeln!(err, "git: {error}");
            return 1;
        }
    };
    if revwalk.push_head().is_err() {
        return 0;
    }
    for (i, oid) in revwalk.enumerate() {
        if i >= n {
            break;
        }
        let oid = match oid {
            Ok(oid) => oid,
            Err(error) => {
                let _ = writeln!(err, "git: {error}");
                return 1;
            }
        };
        let commit = match repo.find_commit(oid) {
            Ok(commit) => commit,
            Err(error) => {
                let _ = writeln!(err, "git: {error}");
                return 1;
            }
        };
        if oneline {
            let _ = writeln!(
                out,
                "{} {}",
                &oid.to_string()[..7],
                commit.summary().ok().flatten().unwrap_or("")
            );
        } else {
            let _ = writeln!(out, "commit {oid}");
            let author = commit.author();
            let _ = writeln!(
                out,
                "Author: {} <{}>",
                author.name().unwrap_or(""),
                author.email().unwrap_or("")
            );
            let _ = writeln!(out, "\n    {}\n", commit.message().unwrap_or("").trim());
        }
    }
    0
}

fn git_diff(repo: &Repository, args: &[String], out: &mut impl Write, err: &mut impl Write) -> u8 {
    let cached = args
        .iter()
        .any(|arg| arg == "--cached" || arg == "--staged");
    let name_only = args.iter().any(|arg| arg == "--name-only");
    let stat = args.iter().any(|arg| arg == "--stat");
    let mut options = git2::DiffOptions::new();
    let separator = args.iter().position(|arg| arg == "--");
    if let Some(separator) = separator {
        for path in &args[separator + 1..] {
            options.pathspec(path);
        }
    }
    let result = if cached {
        let head_tree = repo.head().ok().and_then(|head| head.peel_to_tree().ok());
        repo.diff_tree_to_index(head_tree.as_ref(), None, Some(&mut options))
    } else {
        repo.diff_index_to_workdir(None, Some(&mut options))
    };
    let diff = match result {
        Ok(diff) => diff,
        Err(error) => {
            let _ = writeln!(err, "git: {error}");
            return 1;
        }
    };
    if stat {
        return match diff
            .stats()
            .and_then(|stats| stats.to_buf(git2::DiffStatsFormat::FULL, 80))
        {
            Ok(buffer) => {
                let _ = out.write_all(buffer.as_ref());
                0
            }
            Err(error) => {
                let _ = writeln!(err, "git: {error}");
                1
            }
        };
    }
    let format = if name_only {
        git2::DiffFormat::NameOnly
    } else {
        git2::DiffFormat::Patch
    };
    let result = diff.print(format, |_delta, _hunk, line| {
        let origin = line.origin();
        if !name_only && matches!(origin, '+' | '-' | ' ') {
            let _ = write!(out, "{origin}");
        }
        let _ = out.write_all(line.content());
        true
    });
    match result {
        Ok(()) => 0,
        Err(error) => {
            let _ = writeln!(err, "git: {error}");
            1
        }
    }
}

fn git_tag(repo: &Repository, args: &[String], out: &mut impl Write, err: &mut impl Write) -> u8 {
    if args.is_empty() || args.iter().any(|arg| arg == "-l" || arg == "--list") {
        return match repo.tag_names(None) {
            Ok(names) => {
                for name in names.iter().filter_map(|name| name.ok().flatten()) {
                    let _ = writeln!(out, "{name}");
                }
                0
            }
            Err(error) => {
                let _ = writeln!(err, "git: {error}");
                1
            }
        };
    }
    if let Some(index) = args.iter().position(|arg| arg == "-d" || arg == "--delete") {
        let Some(name) = args.get(index + 1) else {
            let _ = writeln!(err, "git: tag --delete requires a tag name");
            return 2;
        };
        return match repo.tag_delete(name) {
            Ok(()) => {
                let _ = writeln!(out, "Deleted tag '{name}'");
                0
            }
            Err(error) => {
                let _ = writeln!(err, "git: {error}");
                1
            }
        };
    }
    let annotated = args.iter().any(|arg| arg == "-a" || arg == "--annotate");
    let message = parse_flag(args, "-m").or_else(|| parse_flag(args, "--message"));
    let positionals: Vec<&str> = args
        .iter()
        .enumerate()
        .filter(|(index, arg)| {
            !arg.starts_with('-')
                && index
                    .checked_sub(1)
                    .and_then(|previous| args.get(previous))
                    .is_none_or(|previous| previous != "-m" && previous != "--message")
        })
        .map(|(_, arg)| arg.as_str())
        .collect();
    let Some(name) = positionals.first() else {
        let _ = writeln!(err, "git: tag requires a tag name");
        return 2;
    };
    let target_spec = positionals.get(1).copied().unwrap_or("HEAD");
    let target = match repo.revparse_single(target_spec) {
        Ok(target) => target,
        Err(error) => {
            let _ = writeln!(err, "git: {error}");
            return 1;
        }
    };
    let result = if annotated || message.is_some() {
        signature(repo).and_then(|signature| {
            repo.tag(
                name,
                &target,
                &signature,
                message.as_deref().unwrap_or(""),
                false,
            )
            .map(|_| ())
        })
    } else {
        repo.tag_lightweight(name, &target, false).map(|_| ())
    };
    match result {
        Ok(()) => 0,
        Err(error) => {
            let _ = writeln!(err, "git: {error}");
            1
        }
    }
}

fn git_show(repo: &Repository, spec: &str, out: &mut impl Write, err: &mut impl Write) -> u8 {
    let commit = match repo
        .revparse_single(spec)
        .and_then(|object| object.peel_to_commit())
    {
        Ok(commit) => commit,
        Err(error) => {
            let _ = writeln!(err, "git: {error}");
            return 1;
        }
    };
    let _ = writeln!(out, "commit {}", commit.id());
    let author = commit.author();
    let _ = writeln!(
        out,
        "Author: {} <{}>\n\n    {}\n",
        author.name().unwrap_or(""),
        author.email().unwrap_or(""),
        commit.message().unwrap_or("").trim()
    );
    let tree = match commit.tree() {
        Ok(tree) => tree,
        Err(error) => {
            let _ = writeln!(err, "git: {error}");
            return 1;
        }
    };
    let parent_tree = commit.parent(0).ok().and_then(|parent| parent.tree().ok());
    let diff = match repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), None) {
        Ok(diff) => diff,
        Err(error) => {
            let _ = writeln!(err, "git: {error}");
            return 1;
        }
    };
    match diff.print(git2::DiffFormat::Patch, |_delta, _hunk, line| {
        let origin = line.origin();
        if matches!(origin, '+' | '-' | ' ') {
            let _ = write!(out, "{origin}");
        }
        let _ = out.write_all(line.content());
        true
    }) {
        Ok(()) => 0,
        Err(error) => {
            let _ = writeln!(err, "git: {error}");
            1
        }
    }
}

fn git_reset(repo: &Repository, args: &[String], err: &mut impl Write) -> u8 {
    let kind = if args.iter().any(|arg| arg == "--hard") {
        ResetType::Hard
    } else if args.iter().any(|arg| arg == "--soft") {
        ResetType::Soft
    } else {
        ResetType::Mixed
    };
    let target = args
        .iter()
        .find(|arg| !arg.starts_with('-'))
        .map_or("HEAD", String::as_str);
    match repo
        .revparse_single(target)
        .and_then(|object| repo.reset(&object, kind, None))
    {
        Ok(()) => 0,
        Err(error) => {
            let _ = writeln!(err, "git: {error}");
            1
        }
    }
}

fn git_merge(repo: &Repository, args: &[String], out: &mut impl Write, err: &mut impl Write) -> u8 {
    if args.iter().any(|arg| arg == "--abort") {
        let result = repo
            .head()
            .and_then(|head| head.peel(git2::ObjectType::Commit))
            .and_then(|head| repo.reset(&head, ResetType::Hard, None))
            .and_then(|()| repo.cleanup_state());
        return result.map_or_else(
            |error| {
                let _ = writeln!(err, "git: merge --abort failed: {error}");
                1
            },
            |()| 0,
        );
    }
    let no_commit = args.iter().any(|arg| arg == "--no-commit");
    let message = parse_flag(args, "-m").or_else(|| parse_flag(args, "--message"));
    let target_name = args.iter().enumerate().find_map(|(index, arg)| {
        if arg.starts_with('-') {
            None
        } else if index
            .checked_sub(1)
            .and_then(|previous| args.get(previous))
            .is_some_and(|previous| previous == "-m" || previous == "--message")
        {
            None
        } else {
            Some(arg.as_str())
        }
    });
    let Some(target_name) = target_name else {
        let _ = writeln!(err, "git: merge requires a commit or branch");
        return 2;
    };
    let target = match repo.revparse_single(target_name) {
        Ok(target) => target,
        Err(error) => {
            let _ = writeln!(err, "git: {error}");
            return 1;
        }
    };
    let annotated = match repo.find_annotated_commit(target.id()) {
        Ok(annotated) => annotated,
        Err(error) => {
            let _ = writeln!(err, "git: {error}");
            return 1;
        }
    };
    let (analysis, _) = match repo.merge_analysis(&[&annotated]) {
        Ok(analysis) => analysis,
        Err(error) => {
            let _ = writeln!(err, "git: {error}");
            return 1;
        }
    };
    if analysis.is_up_to_date() {
        let _ = writeln!(out, "Already up to date.");
        return 0;
    }
    if analysis.is_fast_forward() {
        let mut head = match repo.head() {
            Ok(head) => head,
            Err(error) => {
                let _ = writeln!(err, "git: {error}");
                return 1;
            }
        };
        if let Err(error) = head
            .set_target(target.id(), &format!("merge {target_name}: Fast-forward"))
            .and_then(|_| repo.checkout_head(Some(git2::build::CheckoutBuilder::default().force())))
        {
            let _ = writeln!(err, "git: merge failed: {error}");
            return 1;
        }
        let _ = writeln!(out, "Fast-forward");
        return 0;
    }
    if let Err(error) = repo.merge(&[&annotated], None, None) {
        let _ = writeln!(err, "git: merge failed: {error}");
        return 1;
    }
    let mut index = match repo.index() {
        Ok(index) => index,
        Err(error) => {
            let _ = writeln!(err, "git: {error}");
            return 1;
        }
    };
    if index.has_conflicts() {
        let _ = writeln!(
            err,
            "Automatic merge failed; fix conflicts and then commit the result."
        );
        return 1;
    }
    if no_commit {
        let _ = writeln!(out, "Automatic merge went well; stopped before committing");
        return 0;
    }
    let tree_id = match index.write_tree() {
        Ok(tree_id) => tree_id,
        Err(error) => {
            let _ = writeln!(err, "git: {error}");
            return 1;
        }
    };
    let result = (|| -> Result<(), git2::Error> {
        let tree = repo.find_tree(tree_id)?;
        let ours = repo.head()?.peel_to_commit()?;
        let theirs = target.peel_to_commit()?;
        let signature = signature(repo)?;
        let message = message.unwrap_or_else(|| format!("Merge branch '{target_name}'"));
        repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            &message,
            &tree,
            &[&ours, &theirs],
        )?;
        repo.cleanup_state()?;
        Ok(())
    })();
    result.map_or_else(
        |error| {
            let _ = writeln!(err, "git: merge commit failed: {error}");
            1
        },
        |()| {
            let _ = writeln!(out, "Merge made by the 'ort' strategy.");
            0
        },
    )
}

fn git_replay_commit(
    repo: &Repository,
    args: &[String],
    revert: bool,
    out: &mut impl Write,
    err: &mut impl Write,
) -> u8 {
    let command = if revert { "revert" } else { "cherry-pick" };
    if args.iter().any(|arg| arg == "--abort") {
        let result = repo
            .head()
            .and_then(|head| head.peel(git2::ObjectType::Commit))
            .and_then(|head| repo.reset(&head, ResetType::Hard, None))
            .and_then(|()| repo.cleanup_state());
        return result.map_or_else(
            |error| {
                let _ = writeln!(err, "git: {command} --abort failed: {error}");
                1
            },
            |()| 0,
        );
    }
    if args.iter().any(|arg| arg == "--continue") {
        let state_file = repo.path().join(if revert {
            "REVERT_HEAD"
        } else {
            "CHERRY_PICK_HEAD"
        });
        let target = match std::fs::read_to_string(&state_file)
            .map_err(|error| git2::Error::from_str(&error.to_string()))
            .and_then(|value| git2::Oid::from_str(value.trim()))
            .and_then(|oid| repo.find_commit(oid))
        {
            Ok(commit) => commit,
            Err(error) => {
                let _ = writeln!(err, "git: no {command} in progress: {error}");
                return 1;
            }
        };
        let index = match repo.index() {
            Ok(index) => index,
            Err(error) => {
                let _ = writeln!(err, "git: {error}");
                return 1;
            }
        };
        if index.has_conflicts() {
            let _ = writeln!(
                err,
                "git: cannot continue {command}: unresolved conflicts remain"
            );
            return 1;
        }
        return finish_replay(repo, &target, revert, out, err);
    }
    let no_commit = args.iter().any(|arg| arg == "-n" || arg == "--no-commit");
    let target_name = args.iter().find(|arg| !arg.starts_with('-'));
    let Some(target_name) = target_name else {
        let _ = writeln!(err, "git: {command} requires a commit");
        return 2;
    };
    let commit = match repo
        .revparse_single(target_name)
        .and_then(|object| object.peel_to_commit())
    {
        Ok(commit) => commit,
        Err(error) => {
            let _ = writeln!(err, "git: {error}");
            return 1;
        }
    };
    let operation = if revert {
        repo.revert(&commit, None)
    } else {
        repo.cherrypick(&commit, None)
    };
    if let Err(error) = operation {
        let _ = writeln!(err, "git: {command} failed: {error}");
        return 1;
    }
    let index = match repo.index() {
        Ok(index) => index,
        Err(error) => {
            let _ = writeln!(err, "git: {error}");
            return 1;
        }
    };
    if index.has_conflicts() {
        let _ = writeln!(
            err,
            "error: could not {command} {target_name}; resolve conflicts and continue or abort"
        );
        return 1;
    }
    if no_commit {
        let _ = repo.cleanup_state();
        return 0;
    }
    finish_replay(repo, &commit, revert, out, err)
}

fn finish_replay(
    repo: &Repository,
    commit: &git2::Commit<'_>,
    revert: bool,
    out: &mut impl Write,
    err: &mut impl Write,
) -> u8 {
    let command = if revert { "revert" } else { "cherry-pick" };
    let tree_id = match repo.index().and_then(|mut index| index.write_tree()) {
        Ok(tree_id) => tree_id,
        Err(error) => {
            let _ = writeln!(err, "git: {error}");
            return 1;
        }
    };
    let result = (|| -> Result<git2::Oid, git2::Error> {
        let tree = repo.find_tree(tree_id)?;
        let parent = repo.head()?.peel_to_commit()?;
        let signature = signature(repo)?;
        let message = if revert {
            format!(
                "Revert \"{}\"\n\nThis reverts commit {}.",
                commit.summary().ok().flatten().unwrap_or(""),
                commit.id()
            )
        } else {
            commit.message().unwrap_or("").to_string()
        };
        let oid = repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            &message,
            &tree,
            &[&parent],
        )?;
        repo.cleanup_state()?;
        Ok(oid)
    })();
    result.map_or_else(
        |error| {
            let _ = writeln!(err, "git: {command} commit failed: {error}");
            1
        },
        |oid| {
            let subject = if revert {
                format!(
                    "Revert \"{}\"",
                    commit.summary().ok().flatten().unwrap_or("")
                )
            } else {
                commit.summary().ok().flatten().unwrap_or("").to_string()
            };
            let _ = writeln!(out, "[{}] {subject}", &oid.to_string()[..7]);
            0
        },
    )
}

fn git_rebase(
    repo: &mut Repository,
    args: &[String],
    out: &mut impl Write,
    err: &mut impl Write,
) -> u8 {
    if args.iter().any(|arg| arg == "--abort") {
        return match repo.open_rebase(None).and_then(|mut rebase| rebase.abort()) {
            Ok(()) => restore_rebase_autostash(repo, out, err),
            Err(error) => {
                let _ = writeln!(err, "git: rebase --abort failed: {error}");
                1
            }
        };
    }
    if args.iter().any(|arg| arg == "--continue") {
        let mut rebase = match repo.open_rebase(None) {
            Ok(rebase) => rebase,
            Err(error) => {
                let _ = writeln!(err, "git: no rebase in progress: {error}");
                return 1;
            }
        };
        if repo.index().is_ok_and(|index| index.has_conflicts()) {
            let _ = writeln!(
                err,
                "git: cannot continue rebase: unresolved conflicts remain"
            );
            return 1;
        }
        let signature = match signature(repo) {
            Ok(signature) => signature,
            Err(error) => {
                let _ = writeln!(err, "git: {error}");
                return 1;
            }
        };
        if let Err(error) = rebase.commit(None, &signature, None) {
            if error.code() != git2::ErrorCode::Applied {
                let _ = writeln!(err, "git: rebase --continue failed: {error}");
                return 1;
            }
        }
        let code = drive_rebase(repo, rebase, &signature, out, err);
        return if code == 0 {
            restore_rebase_autostash(repo, out, err)
        } else {
            code
        };
    }
    let upstream_name = args.iter().find(|arg| !arg.starts_with('-'));
    let Some(upstream_name) = upstream_name else {
        let _ = writeln!(err, "git: rebase requires an upstream");
        return 2;
    };
    let upstream = match repo
        .revparse_single(upstream_name)
        .and_then(|object| repo.find_annotated_commit(object.id()))
    {
        Ok(upstream) => upstream,
        Err(error) => {
            let _ = writeln!(err, "git: {error}");
            return 1;
        }
    };
    let rebase = match repo.rebase(None, Some(&upstream), None, None) {
        Ok(rebase) => rebase,
        Err(error) => {
            let _ = writeln!(err, "git: cannot start rebase: {error}");
            return 1;
        }
    };
    let signature = match signature(repo) {
        Ok(signature) => signature,
        Err(error) => {
            let _ = writeln!(err, "git: {error}");
            return 1;
        }
    };
    let code = drive_rebase(repo, rebase, &signature, out, err);
    drop(upstream);
    if code == 0 {
        restore_rebase_autostash(repo, out, err)
    } else {
        code
    }
}

fn rebase_autostash_marker(repo: &Repository) -> std::path::PathBuf {
    repo.path().join("YOURSHELL_REBASE_AUTOSTASH")
}

fn restore_rebase_autostash(
    repo: &mut Repository,
    out: &mut impl Write,
    err: &mut impl Write,
) -> u8 {
    let marker = rebase_autostash_marker(repo);
    if !marker.exists() {
        return 0;
    }
    if let Err(error) = repo.stash_apply(0, None) {
        let _ = writeln!(
            err,
            "Applying autostash resulted in conflicts; the stash entry is kept: {error}"
        );
        return 1;
    }
    if let Err(error) = repo.stash_drop(0) {
        let _ = writeln!(err, "git: could not drop applied autostash: {error}");
        return 1;
    }
    let _ = std::fs::remove_file(marker);
    let _ = writeln!(out, "Applied autostash.");
    0
}

fn drive_rebase(
    repo: &Repository,
    mut rebase: git2::Rebase<'_>,
    signature: &Signature<'_>,
    out: &mut impl Write,
    err: &mut impl Write,
) -> u8 {
    while let Some(operation) = rebase.next() {
        if let Err(error) = operation {
            let _ = writeln!(err, "git: rebase failed: {error}");
            return 1;
        }
        if repo.index().is_ok_and(|index| index.has_conflicts()) {
            let _ = writeln!(
                err,
                "Resolve all conflicts manually, mark them as resolved, then run 'git rebase --continue'."
            );
            return 1;
        }
        if let Err(error) = rebase.commit(None, signature, None) {
            if error.code() != git2::ErrorCode::Applied {
                let _ = writeln!(err, "git: rebase commit failed: {error}");
                return 1;
            }
        }
    }
    match rebase.finish(Some(signature)) {
        Ok(()) => {
            let _ = writeln!(out, "Successfully rebased and updated HEAD.");
            0
        }
        Err(error) => {
            let _ = writeln!(err, "git: rebase finish failed: {error}");
            1
        }
    }
}

fn git_stash(
    repo: &mut Repository,
    args: &[String],
    out: &mut impl Write,
    err: &mut impl Write,
) -> u8 {
    let subcommand = args.first().map_or("push", String::as_str);
    match subcommand {
        "list" => {
            let result = repo.stash_foreach(|index, message, oid| {
                let _ = writeln!(out, "stash@{{{index}}}: {message} ({oid})");
                true
            });
            result.map_or_else(
                |error| {
                    let _ = writeln!(err, "git: stash list failed: {error}");
                    1
                },
                |()| 0,
            )
        }
        "apply" | "pop" => {
            let index = args
                .iter()
                .skip(1)
                .find(|arg| !arg.starts_with('-'))
                .and_then(|value| parse_stash_index(value))
                .unwrap_or(0);
            let reinstate = args.iter().any(|arg| arg == "--index");
            let mut options = git2::StashApplyOptions::new();
            if reinstate {
                options.reinstantiate_index();
            }
            let result = if subcommand == "pop" {
                repo.stash_pop(index, Some(&mut options))
            } else {
                repo.stash_apply(index, Some(&mut options))
            };
            result.map_or_else(
                |error| {
                    let _ = writeln!(err, "git: stash {subcommand} failed: {error}");
                    1
                },
                |()| 0,
            )
        }
        "drop" => {
            let index = args
                .get(1)
                .and_then(|value| parse_stash_index(value))
                .unwrap_or(0);
            repo.stash_drop(index).map_or_else(
                |error| {
                    let _ = writeln!(err, "git: stash drop failed: {error}");
                    1
                },
                |()| {
                    let _ = writeln!(out, "Dropped stash@{{{index}}}");
                    0
                },
            )
        }
        "clear" => {
            let mut count = 0usize;
            if let Err(error) = repo.stash_foreach(|_, _, _| {
                count += 1;
                true
            }) {
                let _ = writeln!(err, "git: stash clear failed: {error}");
                return 1;
            }
            for _ in 0..count {
                if let Err(error) = repo.stash_drop(0) {
                    let _ = writeln!(err, "git: stash clear failed: {error}");
                    return 1;
                }
            }
            0
        }
        "push" | "save" => {
            let mut flags = git2::StashFlags::DEFAULT;
            if args
                .iter()
                .any(|arg| arg == "-u" || arg == "--include-untracked")
            {
                flags |= git2::StashFlags::INCLUDE_UNTRACKED;
            }
            if args.iter().any(|arg| arg == "-a" || arg == "--all") {
                flags |= git2::StashFlags::INCLUDE_UNTRACKED;
                flags |= git2::StashFlags::INCLUDE_IGNORED;
            }
            if args.iter().any(|arg| arg == "--keep-index") {
                flags |= git2::StashFlags::KEEP_INDEX;
            }
            let message = parse_flag(args, "-m")
                .or_else(|| parse_flag(args, "--message"))
                .or_else(|| {
                    (subcommand == "save")
                        .then(|| {
                            args.iter()
                                .skip(1)
                                .find(|arg| !arg.starts_with('-'))
                                .cloned()
                        })
                        .flatten()
                });
            let signature = match signature(repo) {
                Ok(signature) => signature,
                Err(error) => {
                    let _ = writeln!(err, "git: stash failed: {error}");
                    return 1;
                }
            };
            repo.stash_save2(&signature, message.as_deref(), Some(flags))
                .map_or_else(
                    |error| {
                        let _ = writeln!(err, "git: stash failed: {error}");
                        1
                    },
                    |_| {
                        let _ = writeln!(out, "Saved working directory and index state");
                        0
                    },
                )
        }
        other => {
            let _ = writeln!(err, "git: stash subcommand '{other}' is not supported");
            2
        }
    }
}

fn parse_stash_index(value: &str) -> Option<usize> {
    if let Ok(index) = value.parse::<usize>() {
        return Some(index);
    }
    value
        .strip_prefix("stash@{")
        .and_then(|value| value.strip_suffix('}'))
        .and_then(|value| value.parse().ok())
}

fn git_rm(repo: &Repository, args: &[String], cwd: &Path, err: &mut impl Write) -> u8 {
    let cached = args.iter().any(|arg| arg == "--cached");
    let paths: Vec<&String> = args.iter().filter(|arg| !arg.starts_with('-')).collect();
    if paths.is_empty() {
        let _ = writeln!(err, "git: rm requires a path");
        return 2;
    }
    let mut index = match repo.index() {
        Ok(index) => index,
        Err(error) => {
            let _ = writeln!(err, "git: {error}");
            return 1;
        }
    };
    for path in paths {
        let absolute = resolve_path(cwd, path);
        let relative = repo_relative_path(repo, cwd, path);
        if let Err(error) = index.remove_path(&relative) {
            let _ = writeln!(err, "git: cannot remove '{path}': {error}");
            return 1;
        }
        if !cached {
            let result = if absolute.is_dir() {
                std::fs::remove_dir_all(&absolute)
            } else {
                std::fs::remove_file(&absolute)
            };
            if let Err(error) = result {
                let _ = writeln!(err, "git: cannot remove '{path}': {error}");
                return 1;
            }
        }
    }
    index.write().map_or_else(
        |error| {
            let _ = writeln!(err, "git: {error}");
            1
        },
        |()| 0,
    )
}

fn git_mv(repo: &Repository, args: &[String], cwd: &Path, err: &mut impl Write) -> u8 {
    let paths: Vec<&String> = args.iter().filter(|arg| !arg.starts_with('-')).collect();
    if paths.len() != 2 {
        let _ = writeln!(err, "git: mv requires a source and destination");
        return 2;
    }
    let source = resolve_path(cwd, paths[0]);
    let destination = resolve_path(cwd, paths[1]);
    let source_relative = repo_relative_path(repo, cwd, paths[0]);
    let destination_relative = repo_relative_path(repo, cwd, paths[1]);
    if let Err(error) = std::fs::rename(&source, &destination) {
        let _ = writeln!(err, "git: cannot move '{}': {error}", paths[0]);
        return 1;
    }
    let mut index = match repo.index() {
        Ok(index) => index,
        Err(error) => {
            let _ = writeln!(err, "git: {error}");
            return 1;
        }
    };
    if let Err(error) = index
        .remove_path(&source_relative)
        .and_then(|()| index.add_path(&destination_relative))
        .and_then(|()| index.write())
    {
        let _ = writeln!(err, "git: {error}");
        return 1;
    }
    0
}

fn repo_relative_path(repo: &Repository, cwd: &Path, path: &str) -> std::path::PathBuf {
    let Some(workdir) = repo.workdir() else {
        return Path::new(path).to_path_buf();
    };
    let canonical_workdir =
        std::fs::canonicalize(workdir).unwrap_or_else(|_| workdir.to_path_buf());
    let canonical_cwd = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    canonical_cwd.strip_prefix(&canonical_workdir).map_or_else(
        |_| Path::new(path).to_path_buf(),
        |prefix| prefix.join(path),
    )
}

fn git_checkout(
    repo: &Repository,
    target: &str,
    create: bool,
    out: &mut impl Write,
) -> Result<(), git2::Error> {
    if create {
        let head = repo.head()?.peel_to_commit()?;
        repo.branch(target, &head, false)?;
    }
    let (obj, reference) = repo.revparse_ext(target)?;
    repo.checkout_tree(&obj, None)?;
    if let Some(r) = reference {
        if let Ok(name) = r.name() {
            repo.set_head(name)?;
        }
    } else {
        repo.set_head_detached(obj.id())?;
    }
    let _ = writeln!(out, "Switched to '{target}'");
    Ok(())
}

fn git_fetch(
    repo: &mut Repository,
    args: &[String],
    merge: bool,
    credentials: GitCredentials,
    out: &mut impl Write,
    err: &mut impl Write,
) -> u8 {
    let mut depth = None;
    let mut prune = false;
    let mut tags = None;
    let mut ff_only = false;
    let mut rebase = false;
    let mut rebase_explicit = false;
    let mut autostash = false;
    let mut autostash_explicit = false;
    let mut positionals = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--depth" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    let _ = writeln!(err, "git: --depth requires a value");
                    return 2;
                };
                match value.parse::<i32>() {
                    Ok(value) if value > 0 => depth = Some(value),
                    _ => {
                        let _ = writeln!(err, "git: depth must be a positive integer");
                        return 2;
                    }
                }
            }
            option if option.starts_with("--depth=") => {
                match option.trim_start_matches("--depth=").parse::<i32>() {
                    Ok(value) if value > 0 => depth = Some(value),
                    _ => {
                        let _ = writeln!(err, "git: depth must be a positive integer");
                        return 2;
                    }
                }
            }
            "--prune" | "-p" => prune = true,
            "--tags" => tags = Some(git2::AutotagOption::All),
            "--no-tags" => tags = Some(git2::AutotagOption::None),
            "--ff-only" => ff_only = true,
            "--no-rebase" | "--rebase=false" => rebase_explicit = true,
            "--rebase" | "--rebase=true" => {
                rebase = true;
                rebase_explicit = true;
            }
            "--autostash" => {
                autostash = true;
                autostash_explicit = true;
            }
            option if option.starts_with("--rebase=") => {
                let _ = writeln!(
                    err,
                    "git: pull mode '{}' is not supported; use --rebase",
                    option.trim_start_matches("--rebase=")
                );
                return 2;
            }
            "-q" | "--quiet" | "-f" | "--force" => {}
            option if option.starts_with('-') => {
                let _ = writeln!(err, "git: fetch: unsupported option '{option}'");
                return 2;
            }
            value => positionals.push(value.to_string()),
        }
        index += 1;
    }
    if merge && !rebase_explicit {
        let branch_key = current_branch(repo).map(|branch| format!("branch.{branch}.rebase"));
        rebase = repo.config().is_ok_and(|config| {
            branch_key
                .as_deref()
                .and_then(|key| config.get_bool(key).ok())
                .or_else(|| config.get_bool("pull.rebase").ok())
                .unwrap_or(false)
        });
    }
    if merge && rebase && !autostash_explicit {
        autostash = repo
            .config()
            .ok()
            .and_then(|config| config.get_bool("rebase.autostash").ok())
            .unwrap_or(false);
    }
    let remote_name = positionals.first().map_or("origin", String::as_str);
    let refspecs: Vec<&str> = positionals.iter().skip(1).map(String::as_str).collect();
    if autostash && !(merge && rebase) {
        let _ = writeln!(err, "git: --autostash requires pull --rebase");
        return 2;
    }
    if autostash {
        let tracked_dirty = repo.statuses(None).is_ok_and(|statuses| {
            statuses.iter().any(|entry| {
                let status = entry.status();
                !status.is_wt_new() || status.is_index_new()
            })
        });
        if tracked_dirty {
            let signature = match signature(repo) {
                Ok(signature) => signature,
                Err(error) => {
                    let _ = writeln!(err, "git: cannot create autostash: {error}");
                    return 1;
                }
            };
            if let Err(error) = repo.stash_save2(
                &signature,
                Some("autostash"),
                Some(git2::StashFlags::DEFAULT),
            ) {
                let _ = writeln!(err, "git: cannot create autostash: {error}");
                return 1;
            }
            if let Err(error) = std::fs::write(rebase_autostash_marker(repo), b"stash@{0}\n") {
                let _ = writeln!(err, "git: cannot record autostash: {error}");
                return 1;
            }
            let _ = writeln!(out, "Created autostash.");
        }
    }
    let mut remote = match repo.find_remote(remote_name) {
        Ok(r) => r,
        Err(e) => {
            let _ = writeln!(err, "git: {e}");
            return 1;
        }
    };
    let mut fo = git2::FetchOptions::new();
    fo.remote_callbacks(cred_callback(credentials));
    if let Some(depth) = depth {
        fo.depth(depth);
    }
    if prune {
        fo.prune(git2::FetchPrune::On);
    }
    if let Some(tags) = tags {
        fo.download_tags(tags);
    }
    if let Err(e) = remote.fetch(&refspecs, Some(&mut fo), None) {
        let _ = writeln!(err, "git: fetch failed: {e}");
        drop(remote);
        if autostash {
            let _ = restore_rebase_autostash(repo, out, err);
        }
        return 1;
    }
    drop(remote);
    let _ = writeln!(out, "Fetched {remote_name}");
    if merge {
        // Fast-forward merge of the upstream of the current branch, if possible.
        let fetched_id = repo
            .find_reference("FETCH_HEAD")
            .ok()
            .and_then(|reference| repo.reference_to_annotated_commit(&reference).ok())
            .map(|commit| commit.id());
        if let Some(fetched_id) = fetched_id {
            let analysis = repo
                .find_annotated_commit(fetched_id)
                .and_then(|commit| repo.merge_analysis(&[&commit]).map(|value| value.0));
            if let Ok(analysis) = analysis {
                if analysis.is_fast_forward() {
                    if let Ok(mut head_ref) = repo.head() {
                        let _ = head_ref.set_target(fetched_id, "pull: fast-forward");
                        let _ = repo
                            .checkout_head(Some(git2::build::CheckoutBuilder::default().force()));
                        let _ = writeln!(out, "Fast-forwarded to {fetched_id}");
                    }
                } else if analysis.is_up_to_date() {
                    let _ = writeln!(out, "Already up to date");
                } else if rebase {
                    return git_rebase(repo, &[fetched_id.to_string()], out, err);
                } else if ff_only {
                    let _ = writeln!(err, "fatal: Not possible to fast-forward, aborting.");
                    return 1;
                } else {
                    return git_merge(repo, &[fetched_id.to_string()], out, err);
                }
            }
        }
    }
    if autostash {
        restore_rebase_autostash(repo, out, err)
    } else {
        0
    }
}

fn git_config(
    mut config: git2::Config,
    args: &[String],
    out: &mut impl Write,
    err: &mut impl Write,
) -> u8 {
    if args.iter().any(|arg| arg == "--global") {
        config = match config.open_global() {
            Ok(config) => config,
            Err(error) => {
                let _ = writeln!(err, "fatal: unable to open global config: {error}");
                return 1;
            }
        };
    }
    let list = args.iter().any(|arg| arg == "-l" || arg == "--list");
    let get = args.iter().any(|arg| arg == "--get");
    let unset = args.iter().any(|arg| arg == "--unset");
    let positionals: Vec<&str> = args
        .iter()
        .filter(|arg| !arg.starts_with('-'))
        .map(String::as_str)
        .collect();
    if list {
        let mut entries = match config.entries(None) {
            Ok(entries) => entries,
            Err(error) => {
                let _ = writeln!(err, "error: {error}");
                return 1;
            }
        };
        while let Some(Ok(entry)) = entries.next() {
            if let (Ok(name), Ok(value)) = (entry.name(), entry.value()) {
                let _ = writeln!(out, "{name}={value}");
            }
        }
        return 0;
    }
    if unset {
        let Some(key) = positionals.first() else {
            let _ = writeln!(err, "usage: git config --unset <name>");
            return 2;
        };
        return match config.remove(key) {
            Ok(()) => 0,
            Err(_) => 5,
        };
    }
    if get || positionals.len() == 1 {
        let Some(key) = positionals.first() else {
            let _ = writeln!(err, "usage: git config --get <name>");
            return 2;
        };
        return match config.get_string(key) {
            Ok(value) => {
                let _ = writeln!(out, "{value}");
                0
            }
            Err(_) => 1,
        };
    }
    if positionals.len() == 2 {
        return match config.set_str(positionals[0], positionals[1]) {
            Ok(()) => 0,
            Err(error) => {
                let _ = writeln!(err, "error: {error}");
                1
            }
        };
    }
    let _ = writeln!(err, "usage: git config [<options>] [<name> [<value>]]");
    2
}

fn git_remote(
    repo: &Repository,
    args: &[String],
    out: &mut impl Write,
    err: &mut impl Write,
) -> u8 {
    let verbose = args.iter().any(|arg| arg == "-v" || arg == "--verbose");
    let values: Vec<&str> = args
        .iter()
        .filter(|arg| !arg.starts_with('-'))
        .map(String::as_str)
        .collect();
    match values.first().copied() {
        Some("add") => {
            if values.len() != 3 {
                let _ = writeln!(err, "usage: git remote add <name> <url>");
                return 2;
            }
            match repo.remote(values[1], values[2]) {
                Ok(_) => 0,
                Err(error) => {
                    let _ = writeln!(err, "error: {error}");
                    1
                }
            }
        }
        Some("remove" | "rm") => {
            if values.len() != 2 {
                let _ = writeln!(err, "usage: git remote remove <name>");
                return 2;
            }
            match repo.remote_delete(values[1]) {
                Ok(()) => 0,
                Err(error) => {
                    let _ = writeln!(err, "error: {error}");
                    1
                }
            }
        }
        Some("rename") => {
            if values.len() != 3 {
                let _ = writeln!(err, "usage: git remote rename <old> <new>");
                return 2;
            }
            match repo.remote_rename(values[1], values[2]) {
                Ok(problems) if problems.is_empty() => 0,
                Ok(problems) => {
                    for problem in problems.iter().filter_map(|value| value.ok().flatten()) {
                        let _ = writeln!(err, "warning: {problem}");
                    }
                    0
                }
                Err(error) => {
                    let _ = writeln!(err, "error: {error}");
                    1
                }
            }
        }
        Some("get-url") => {
            let Some(name) = values.get(1) else {
                let _ = writeln!(err, "usage: git remote get-url <name>");
                return 2;
            };
            match repo.find_remote(name).ok().and_then(|remote| {
                if args.iter().any(|arg| arg == "--push") {
                    remote
                        .pushurl()
                        .ok()
                        .flatten()
                        .map(str::to_string)
                        .or_else(|| remote.url().ok().map(str::to_string))
                } else {
                    remote.url().ok().map(str::to_string)
                }
            }) {
                Some(url) => {
                    let _ = writeln!(out, "{url}");
                    0
                }
                None => {
                    let _ = writeln!(err, "error: No such remote '{name}'");
                    2
                }
            }
        }
        Some("set-url") => {
            if values.len() != 3 {
                let _ = writeln!(err, "usage: git remote set-url <name> <newurl>");
                return 2;
            }
            let result = if args.iter().any(|arg| arg == "--push") {
                repo.remote_set_pushurl(values[1], Some(values[2]))
            } else {
                repo.remote_set_url(values[1], values[2])
            };
            match result {
                Ok(()) => 0,
                Err(error) => {
                    let _ = writeln!(err, "error: {error}");
                    1
                }
            }
        }
        Some(command) => {
            let _ = writeln!(err, "git: remote subcommand '{command}' is not supported");
            2
        }
        None => {
            let remotes = match repo.remotes() {
                Ok(remotes) => remotes,
                Err(error) => {
                    let _ = writeln!(err, "error: {error}");
                    return 1;
                }
            };
            for name in remotes.iter().filter_map(|name| name.ok().flatten()) {
                if verbose {
                    if let Ok(remote) = repo.find_remote(name) {
                        if let Ok(url) = remote.url() {
                            let _ = writeln!(out, "{name}\t{url} (fetch)");
                        }
                        let push_url = remote
                            .pushurl()
                            .ok()
                            .flatten()
                            .or_else(|| remote.url().ok());
                        if let Some(url) = push_url {
                            let _ = writeln!(out, "{name}\t{url} (push)");
                        }
                    }
                } else {
                    let _ = writeln!(out, "{name}");
                }
            }
            0
        }
    }
}

fn git_push(
    repo: &Repository,
    rest: &[String],
    credentials: GitCredentials,
    out: &mut impl Write,
    err: &mut impl Write,
) -> u8 {
    let set_upstream = rest
        .iter()
        .any(|arg| arg == "-u" || arg == "--set-upstream");
    let force = rest.iter().any(|arg| arg == "-f" || arg == "--force");
    let force_with_lease = rest.iter().any(|arg| arg == "--force-with-lease");
    for option in rest.iter().filter(|arg| arg.starts_with('-')) {
        if !matches!(
            option.as_str(),
            "-u" | "--set-upstream" | "-f" | "--force" | "--force-with-lease" | "-q" | "--quiet"
        ) {
            let _ = writeln!(err, "git: push unsupported option '{option}'");
            return 2;
        }
    }
    let positionals: Vec<&str> = rest
        .iter()
        .filter(|a| !a.starts_with('-'))
        .map(String::as_str)
        .collect();
    let remote_name = positionals.first().copied().unwrap_or("origin");
    let mut remote = match repo.find_remote(remote_name) {
        Ok(r) => r,
        Err(e) => {
            let _ = writeln!(err, "git: {e}");
            return 1;
        }
    };
    let branch = positionals.get(1).map_or_else(
        || {
            repo.head()
                .ok()
                .and_then(|h| h.shorthand().ok().map(String::from))
                .unwrap_or_else(|| "main".to_string())
        },
        |b| (*b).to_string(),
    );
    if force_with_lease {
        let tracking_ref = format!("refs/remotes/{remote_name}/{branch}");
        let expected = repo.refname_to_id(&tracking_ref).ok();
        let advertised = match remote.connect_auth(
            git2::Direction::Push,
            Some(cred_callback(credentials.clone())),
            None,
        ) {
            Ok(connection) => connection
                .list()
                .ok()
                .and_then(|heads| {
                    heads
                        .iter()
                        .find(|head| head.name() == format!("refs/heads/{branch}"))
                })
                .map(|head| head.oid()),
            Err(error) => {
                let _ = writeln!(err, "git: push lease check failed: {error}");
                return 1;
            }
        };
        if expected != advertised {
            let _ = writeln!(err, "git: push rejected: stale info for '{branch}'");
            return 1;
        }
    }
    let prefix = if force || force_with_lease { "+" } else { "" };
    let refspec = format!("{prefix}refs/heads/{branch}:refs/heads/{branch}");
    let mut po = git2::PushOptions::new();
    po.remote_callbacks(cred_callback(credentials));
    match remote.push(&[&refspec], Some(&mut po)) {
        Ok(()) => {
            if let Ok(head_id) = repo.refname_to_id(&format!("refs/heads/{branch}")) {
                let _ = repo.reference(
                    &format!("refs/remotes/{remote_name}/{branch}"),
                    head_id,
                    true,
                    "update after push",
                );
            }
            if set_upstream {
                if let Ok(mut config) = repo.config() {
                    let _ = config.set_str(&format!("branch.{branch}.remote"), remote_name);
                    let _ = config.set_str(
                        &format!("branch.{branch}.merge"),
                        &format!("refs/heads/{branch}"),
                    );
                }
            }
            let _ = writeln!(out, "Pushed {branch} to {remote_name}");
            0
        }
        Err(e) => {
            let _ = writeln!(err, "git: push failed: {e}");
            1
        }
    }
}

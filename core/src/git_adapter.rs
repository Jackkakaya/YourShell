//! `git` command backed by libgit2 (git2 crate, vendored). In-process, no
//! fork/exec. HTTPS transport (SecureTransport-equivalent via vendored
//! OpenSSL). Covers the common porcelain: init/clone/status/add/commit/log/
//! diff/branch/checkout/pull/push/config/remote. Auth for private repos via
//! GIT_USERNAME + GIT_PASSWORD (or token) env vars.

use std::io::Write;
use std::path::Path;

use brush_core::{builtins, ExecutionContext, ExecutionResult, ShellExtensions};
use git2::{Repository, Signature};

fn open_repo(cwd: &Path) -> Result<Repository, git2::Error> {
    Repository::discover(cwd)
}

fn cred_callback() -> git2::RemoteCallbacks<'static> {
    let mut cb = git2::RemoteCallbacks::new();
    cb.credentials(|_url, username_from_url, _allowed| {
        if let (Ok(user), Ok(pass)) = (std::env::var("GIT_USERNAME"), std::env::var("GIT_PASSWORD"))
        {
            git2::Cred::userpass_plaintext(&user, &pass)
        } else if let Ok(token) = std::env::var("GIT_TOKEN") {
            git2::Cred::userpass_plaintext(
                username_from_url.unwrap_or("git"),
                &token,
            )
        } else {
            git2::Cred::default()
        }
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
        Ok(ExecutionResult::new(code))
    })
}

#[allow(clippy::too_many_lines)]
fn run_git<SE: ShellExtensions>(
    argv: &[String],
    cwd: &Path,
    context: &ExecutionContext<'_, SE>,
) -> u8 {
    let mut out = context.stdout();
    let mut err = context.stderr();
    let Some(sub) = argv.first() else {
        let _ = writeln!(err, "usage: git <command> [args]");
        return 1;
    };
    let rest = &argv[1..];

    macro_rules! fail {
        ($e:expr) => {{
            let _ = writeln!(err, "git: {}", $e);
            return 1;
        }};
    }

    match sub.as_str() {
        "init" => {
            let dir = rest.first().map_or_else(|| cwd.to_path_buf(), |d| cwd.join(d));
            match Repository::init(&dir) {
                Ok(_) => {
                    let _ = writeln!(out, "Initialized empty Git repository in {}", dir.display());
                    0
                }
                Err(e) => fail!(e),
            }
        }
        "clone" => {
            let Some(url) = rest.first() else { fail!("clone requires a URL") };
            let dir = rest.get(1).map_or_else(
                || cwd.join(url_repo_name(url)),
                |d| cwd.join(d),
            );
            let mut fo = git2::FetchOptions::new();
            fo.remote_callbacks(cred_callback());
            let mut builder = git2::build::RepoBuilder::new();
            builder.fetch_options(fo);
            match builder.clone(url, &dir) {
                Ok(_) => {
                    let _ = writeln!(out, "Cloned into '{}'", dir.display());
                    0
                }
                Err(e) => fail!(e),
            }
        }
        "status" => match open_repo(cwd) {
            Ok(repo) => git_status(&repo, &mut out).map_or(1, |()| 0),
            Err(e) => fail!(e),
        },
        "add" => match open_repo(cwd) {
            Ok(repo) => {
                let mut index = match repo.index() {
                    Ok(i) => i,
                    Err(e) => fail!(e),
                };
                for p in rest {
                    if p == "." || p == "-A" {
                        let _ = index.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None);
                    } else {
                        let _ = index.add_path(Path::new(p));
                    }
                }
                let _ = index.write();
                0
            }
            Err(e) => fail!(e),
        },
        "commit" => {
            let msg = parse_flag(rest, "-m").unwrap_or_default();
            if msg.is_empty() {
                fail!("commit requires -m <message>");
            }
            match open_repo(cwd) {
                Ok(repo) => git_commit(&repo, &msg, &mut out).map_or(1, |()| 0),
                Err(e) => fail!(e),
            }
        }
        "log" => match open_repo(cwd) {
            Ok(repo) => {
                let n = parse_flag(rest, "-n")
                    .and_then(|s| s.parse::<usize>().ok())
                    .unwrap_or(20);
                git_log(&repo, n, &mut out).map_or(1, |()| 0)
            }
            Err(e) => fail!(e),
        },
        "diff" => match open_repo(cwd) {
            Ok(repo) => git_diff(&repo, &mut out).map_or(1, |()| 0),
            Err(e) => fail!(e),
        },
        "branch" => match open_repo(cwd) {
            Ok(repo) => {
                if let Some(name) = rest.first().filter(|n| !n.starts_with('-')) {
                    if let Ok(head) = repo.head().and_then(|h| h.peel_to_commit()) {
                        let _ = repo.branch(name, &head, false);
                        let _ = writeln!(out, "Created branch {name}");
                    }
                    0
                } else {
                    if let Ok(branches) = repo.branches(Some(git2::BranchType::Local)) {
                        let cur = repo.head().ok().and_then(|h| h.shorthand().ok().map(String::from));
                        for b in branches.flatten() {
                            if let Ok(Some(name)) = b.0.name() {
                                let marker = if Some(name) == cur.as_deref() { "* " } else { "  " };
                                let _ = writeln!(out, "{marker}{name}");
                            }
                        }
                    }
                    0
                }
            }
            Err(e) => fail!(e),
        },
        "checkout" => match open_repo(cwd) {
            Ok(repo) => {
                let Some(target) = rest.iter().find(|a| !a.starts_with('-')) else {
                    fail!("checkout requires a target");
                };
                git_checkout(&repo, target, rest.contains(&"-b".to_string()), &mut out)
                    .map_or(1, |()| 0)
            }
            Err(e) => fail!(e),
        },
        "pull" | "fetch" => match open_repo(cwd) {
            Ok(repo) => git_fetch(&repo, sub == "pull", &mut out, &mut err),
            Err(e) => fail!(e),
        },
        "push" => match open_repo(cwd) {
            Ok(repo) => git_push(&repo, rest, &mut out, &mut err),
            Err(e) => fail!(e),
        },
        "config" => {
            match open_repo(cwd).and_then(|r| r.config()).or_else(|_| git2::Config::open_default()) {
                Ok(mut cfg) => {
                    if rest.len() >= 2 {
                        let _ = cfg.set_str(&rest[0], &rest[1]);
                    } else if let Some(key) = rest.first() {
                        if let Ok(v) = cfg.get_string(key) {
                            let _ = writeln!(out, "{v}");
                        }
                    }
                    0
                }
                Err(e) => fail!(e),
            }
        }
        "remote" => match open_repo(cwd) {
            Ok(repo) => {
                if rest.first().map(String::as_str) == Some("add") && rest.len() >= 3 {
                    let _ = repo.remote(&rest[1], &rest[2]);
                } else if let Ok(remotes) = repo.remotes() {
                    for r in remotes.iter().filter_map(|x| x.ok().flatten()) {
                        let _ = writeln!(out, "{r}");
                    }
                }
                0
            }
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

fn parse_flag(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1).cloned())
}

fn signature(repo: &Repository) -> Result<Signature<'static>, git2::Error> {
    let cfg = repo.config()?;
    let name = cfg
        .get_string("user.name")
        .or_else(|_| std::env::var("GIT_AUTHOR_NAME").map_err(|_| git2::Error::from_str("no name")))
        .unwrap_or_else(|_| "YourShell User".to_string());
    let email = cfg
        .get_string("user.email")
        .or_else(|_| std::env::var("GIT_AUTHOR_EMAIL").map_err(|_| git2::Error::from_str("no email")))
        .unwrap_or_else(|_| "user@yourshell.local".to_string());
    Signature::now(&name, &email)
}

fn git_status(repo: &Repository, out: &mut impl Write) -> Result<(), git2::Error> {
    let head = repo.head().ok().and_then(|h| h.shorthand().ok().map(String::from));
    let _ = writeln!(out, "On branch {}", head.as_deref().unwrap_or("(no branch)"));
    let statuses = repo.statuses(None)?;
    if statuses.is_empty() {
        let _ = writeln!(out, "nothing to commit, working tree clean");
        return Ok(());
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
    Ok(())
}

fn git_commit(repo: &Repository, msg: &str, out: &mut impl Write) -> Result<(), git2::Error> {
    let sig = signature(repo)?;
    let mut index = repo.index()?;
    let tree_id = index.write_tree()?;
    let tree = repo.find_tree(tree_id)?;
    let parents = repo
        .head()
        .ok()
        .and_then(|h| h.peel_to_commit().ok())
        .into_iter()
        .collect::<Vec<_>>();
    let parent_refs: Vec<&git2::Commit> = parents.iter().collect();
    let oid = repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &parent_refs)?;
    let _ = writeln!(out, "[{}] {}", &oid.to_string()[..7], msg.lines().next().unwrap_or(""));
    Ok(())
}

fn git_log(repo: &Repository, n: usize, out: &mut impl Write) -> Result<(), git2::Error> {
    let mut revwalk = repo.revwalk()?;
    if revwalk.push_head().is_err() {
        return Ok(());
    }
    for (i, oid) in revwalk.enumerate() {
        if i >= n {
            break;
        }
        let oid = oid?;
        let commit = repo.find_commit(oid)?;
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
    Ok(())
}

fn git_diff(repo: &Repository, out: &mut impl Write) -> Result<(), git2::Error> {
    let diff = repo.diff_index_to_workdir(None, None)?;
    diff.print(git2::DiffFormat::Patch, |_delta, _hunk, line| {
        let origin = line.origin();
        if matches!(origin, '+' | '-' | ' ') {
            let _ = write!(out, "{origin}");
        }
        let _ = out.write_all(line.content());
        true
    })?;
    Ok(())
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

fn git_fetch(repo: &Repository, merge: bool, out: &mut impl Write, err: &mut impl Write) -> u8 {
    let mut remote = match repo.find_remote("origin") {
        Ok(r) => r,
        Err(e) => {
            let _ = writeln!(err, "git: {e}");
            return 1;
        }
    };
    let mut fo = git2::FetchOptions::new();
    fo.remote_callbacks(cred_callback());
    if let Err(e) = remote.fetch(&[] as &[&str], Some(&mut fo), None) {
        let _ = writeln!(err, "git: fetch failed: {e}");
        return 1;
    }
    let _ = writeln!(out, "Fetched origin");
    if merge {
        // Fast-forward merge of the upstream of the current branch, if possible.
        if let Ok(fetch_head) = repo.find_reference("FETCH_HEAD") {
            if let Ok(commit) = repo.reference_to_annotated_commit(&fetch_head) {
                let analysis = repo.merge_analysis(&[&commit]);
                if let Ok((a, _)) = analysis {
                    if a.is_fast_forward() {
                        if let Ok(mut head_ref) = repo.head() {
                            let _ = head_ref.set_target(commit.id(), "pull: fast-forward");
                            let _ = repo.checkout_head(Some(
                                git2::build::CheckoutBuilder::default().force(),
                            ));
                            let _ = writeln!(out, "Fast-forwarded to {}", commit.id());
                        }
                    } else if a.is_up_to_date() {
                        let _ = writeln!(out, "Already up to date");
                    } else {
                        let _ = writeln!(err, "git: non-fast-forward merge not supported");
                    }
                }
            }
        }
    }
    0
}

fn git_push(repo: &Repository, rest: &[String], out: &mut impl Write, err: &mut impl Write) -> u8 {
    let remote_name = rest.first().map_or("origin", String::as_str);
    let mut remote = match repo.find_remote(remote_name) {
        Ok(r) => r,
        Err(e) => {
            let _ = writeln!(err, "git: {e}");
            return 1;
        }
    };
    let branch = repo
        .head()
        .ok()
        .and_then(|h| h.shorthand().ok().map(String::from))
        .unwrap_or_else(|| "main".to_string());
    let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");
    let mut po = git2::PushOptions::new();
    po.remote_callbacks(cred_callback());
    match remote.push(&[&refspec], Some(&mut po)) {
        Ok(()) => {
            let _ = writeln!(out, "Pushed {branch} to {remote_name}");
            0
        }
        Err(e) => {
            let _ = writeln!(err, "git: push failed: {e}");
            1
        }
    }
}

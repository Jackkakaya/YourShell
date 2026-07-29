# Git on iOS: implementation and compatibility plan

Last audited: 2026-07-29

## Product decision

YourShell will provide an in-process, Git-compatible CLI backed by
`git2-rs`/libgit2. It will not statically embed upstream `git/git`.

- Upstream Git has 149 compiled builtins plus shell/Perl helpers and is
  GPLv2-only. Embedding it would create licensing and App Store distribution
  risk, and its `fork`/`exec`, pager, editor, helper, hook and process-global
  assumptions do not fit iOS.
- libgit2 1.9.6 has an explicit unlimited linking exception. `git2` 0.21 is
  already integrated and compiles for the iOS simulator.
- `git2/ssh` was cross-compiled successfully for
  `aarch64-apple-ios-sim`; libgit2/libssh2 is therefore the selected SSH
  protocol engine.
- Until the release matrix passes, `git --version` must identify libgit2 and
  the app must not claim to contain the original or complete Git CLI.

The goal is not “149 names that return zero”. The goal is compatible argv,
repository state, output, exit status, authentication, cancellation and
repeated/session-safe execution for the workflows users actually run.

## Current implementation, measured

| Area | Current state | Main gap |
|---|---|---|
| Dispatch | One `match` in `core/src/git_adapter.rs` | Needs command modules/registry |
| Global argv | `-C`, `--git-dir`, `--work-tree`, `--no-pager` | `-c`, help, config-env, namespace |
| Local porcelain | Includes restore/clean and merge/rebase/stash conflict flows | reflog, blame, bisect and flag depth |
| Remote porcelain | clone/fetch/pull/push/ls-remote over HTTPS | refspec edge cases, Keychain UI, progress/cancel |
| HTTPS | vendored libgit2 + OpenSSL | session credentials and private-host matrix |
| SSH | `git2/ssh` enabled and cross-compiled for iOS simulator | Keychain UI and known_hosts policy |
| Tests | Desktop-Git differential state tests; simulator scenarios | systematic argv/output/exit matrix and device protocols |
| Session safety | repository operations and transport credentials use the Brush session | author identity fallback and repository-scoped mutation locks |
| Licensing | libgit2 route is viable | root project license and third-party notices missing |

Implemented top-level commands at this audit:

`init clone ls-remote status add commit log diff branch checkout switch tag
show reset restore clean rev-parse ls-files ls-tree show-ref symbolic-ref
merge-base merge cherry-pick revert stash rebase rm mv fetch pull push config
remote version`.

### Completed in the 2026-07-29 execution batch

- Added `restore`, `clean`, `rev-parse`, `ls-files`, `ls-tree`, `show-ref`,
  `symbolic-ref`, `merge-base` and `ls-remote`.
- Added desktop-Git oracle checks for restore/clean and exact plumbing stdout
  and exit status. The oracle exposed and fixed `--show-toplevel` trailing
  slash and combined `clean -fd` behavior.
- Enabled `git2/ssh`; both normal tests and
  `aarch64-apple-ios-sim` compilation pass.
- Moved HTTPS/SSH credential reads from process-global `std::env` into the
  current Brush shell session. SSH accepts key paths or in-memory private keys.
- Added optional strict SSH SHA-256 host-key pinning through the session
  (`GIT_SSH_HOSTKEY_SHA256`); unpinned connections retain libgit2 verification
  rather than being blindly accepted.
- Verified live HTTPS `ls-remote` and shallow clone against GitHub.
- Full command battery remains 337/337.
- Deepened the most common daily commands:
  - `commit -a/-am/--all` stages tracked modifications and deletions.
  - `branch` supports explicit start points, `-a/-r`, safe `-d`, forced `-D`
    and `-m` rename.
  - `remote` supports `-v`, add/remove/rename/get-url/set-url and push URLs.
  - `log` supports `--oneline`, `-N`, `-n` and `--max-count`.
  - `config` supports set/get/unset/list and global-config selection.
- Added oracle workflows for these commands; Git compatibility tests, the
  337-case battery and the iOS simulator cross-build all pass after the change.
- Completed the remaining high-frequency compatibility batch:
  - `diff --staged/--cached`, `--name-only`, `--stat` and pathspec after `--`.
  - `commit --amend`, `--allow-empty`, and correct failure for an empty normal
    commit.
  - `push -u/--set-upstream`, `-f/--force`, and guarded
    `--force-with-lease`; successful pushes update the remote-tracking ref.
  - `pull --ff-only` and `--no-rebase`; ordinary divergent pulls now create a
    merge commit instead of incorrectly printing an error and returning zero.
- Added paired local bare-remotes to the oracle suite. Tests cover upstream
  configuration, normal push, history rewrite, valid/stale leases,
  fast-forward pull, divergent pull, and ff-only rejection.
- Re-ran the live GitHub HTTPS test, the full 337-case battery, and the iOS
  simulator cross-build after this batch; all pass.
- Completed `pull --rebase` as a daily-use workflow:
  - CLI `--rebase`, `--rebase=true`, `--no-rebase` and `--rebase=false`.
  - Repository configuration through `pull.rebase` and
    `branch.<name>.rebase`.
  - `--autostash` and configured `rebase.autostash`, including persistence
    across a conflict and restoration after `--continue` or `--abort`.
  - Clean linear rebases, no-op/fast-forward behavior, conflict
    continue/abort, empty/already-applied operations and dirty-worktree
    restoration.
- The paired bare-remote oracle exercises each of those lifecycle paths
  against desktop Git. Live HTTPS, 337/337 Battery and the iOS simulator
  cross-build pass after the implementation.

### Deliberate daily-use boundary after this batch

The ordinary local/HTTPS Git workflow is covered. Remaining work is advanced
or interactive rather than a hidden “common command” gap:

- Interactive editor flows (`commit` without `-m`, interactive rebase,
  interactive add) need the iOS editor Host.
- Keychain UI and persistent known_hosts management remain before turnkey SSH
  private-repository UX; SSH keys, passphrases, transport and strict SHA-256
  pinning already exist.
- Submodule, worktree, sparse checkout, LFS, hooks and filters remain P5/P7.

## Upstream command inventory

The 149 upstream builtins are divided by product responsibility. Internal
aliases and transport/server programs are intentionally not counted as
ordinary interactive commands.

| Class | Commands | Delivery |
|---|---|---|
| Daily porcelain | add, branch, checkout, cherry-pick, clean, clone, commit, config, diff, fetch, init, log, merge, mv, pull, push, rebase, remote, reset, restore, revert, rm, show, stash, status, switch, tag | P1/P3; full common CLI contract |
| History/query porcelain | annotate, bisect, blame, describe, grep, log, name-rev, notes, range-diff, reflog, shortlog, show-branch, whatchanged | P2/P5 |
| Script-facing plumbing | apply, cat-file, check-attr, check-ignore, check-mailmap, check-ref-format, commit-tree, diff-files, diff-index, diff-tree, for-each-ref, hash-object, interpret-trailers, ls-files, ls-remote, ls-tree, merge-base, merge-file, merge-tree, mktag, mktree, patch-id, read-tree, rev-list, rev-parse, show-index, show-ref, symbolic-ref, update-index, update-ref, var, verify-commit, verify-pack, verify-tag, write-tree | P2/P4; stable stdout/exit status |
| Repository exchange | archive, bundle, fast-export, fast-import, fetch-pack, index-pack, pack-objects, send-pack, unpack-objects, upload-archive, upload-pack | P5/P6; in-process only |
| Repository administration | commit-graph, count-objects, diagnose, fsck, gc, maintenance, multi-pack-index, pack-redundant, pack-refs, prune, prune-packed, refs, repack, replace, rerere, sparse-checkout, submodule--helper, update-server-info, worktree | P5/P6 |
| Internal aliases/helpers | checkout--worker, checkout-index, column, credential, credential-cache, credential-cache--daemon, credential-store, difftool, fmt-merge-msg, hook, merge-index, merge-ours, merge-recursive, merge-recursive-ours, merge-recursive-theirs, merge-subtree, pickaxe, replay, stage, stripspace, unpack-file, upload-archive--writer, url-parse | Implement only when a public workflow/script requires them |
| Server/legacy/diagnostic | am, backfill, bugreport, help, history, last-modified, mailinfo, mailsplit, receive-pack, repo, version | P7 or explicit documented boundary |

External upstream scripts (`svn`, `p4`, `send-email`, CVS helpers,
`filter-branch`, `instaweb`, mergetool/difftool helpers, etc.) require their
own runtimes or external services. They are not silently emulated.

## Target architecture

```text
Brush session
    |
    v
GitInvocation { argv, cwd, env, stdio, cancellation, credential_host }
    |
    v
declarative command registry
    |
    +-- porcelain modules ---------> git2/libgit2 repository APIs
    +-- plumbing modules ----------> git2/libgit2 object/ref/index APIs
    +-- HTTPS callbacks -----------> session credentials / Keychain
    +-- SSH callbacks -------------> libssh2 + Keychain + known_hosts
    +-- editor/pager/prompt -------> iOS Host UI
    `-- hooks/filters -------------> policy-controlled Brush child command
```

No Git operation may change process cwd or use process-global credentials.
Different Shell sessions may execute concurrently; operations mutating the
same repository require a repository-scoped lock, not one global lock.

## Continuous delivery plan

| Phase | Scope | Required implementation | Done gate |
|---|---|---|---|
| P0 Foundation | Invocation, registry, errors, isolation | `GitInvocation`; per-command modules; shared option primitives; session env/credentials/cancel; repository lock key | repeated and two-session concurrency tests; no process-global credential reads |
| P1 Local Git | Daily local porcelain | finish restore/clean; deepen add/commit/log/diff/branch/checkout/switch/tag/reset/config/remote flags; conflict lifecycle | desktop Git differential state + output/exit fixtures for clean/conflict/abort/continue |
| P2 Script compatibility | Query and plumbing | rev-parse, rev-list, cat-file, hash-object, ls-files, ls-tree, show-ref, symbolic-ref, update-ref, write-tree, commit-tree, merge-base, for-each-ref, check-* | shell-script fixtures run unchanged against desktop Git and YourShell |
| P3 HTTPS remotes | Public/private smart HTTP | ls-remote; clone/fetch/pull/push refspecs; shallow/tags/prune; token/basic auth; TLS errors; progress/cancel | GitHub/GitLab public and private matrix on simulator/device |
| P4 SSH remotes | Private SSH workflows | enable `git2/ssh`; in-memory/file keys; Keychain; passphrase prompt; strict/TOFU known_hosts; cancel/network recovery | clone/fetch/push with RSA/Ed25519 keys on simulator/device |
| P5 Advanced workflows | Modern repository use | worktree, submodule, sparse-checkout, archive, bundle, notes, blame, bisect, rerere | end-to-end fixtures, including nested submodules and worktrees |
| P6 Storage/maintenance | Object database health | fsck/count-objects/gc/repack/prune/pack refs and necessary pack plumbing | corruption fixtures detected; maintenance never loses reachable objects |
| P7 Host integration/boundaries | Interactive/external behavior | editor, pager, credential prompt, hooks/filters policy; explicit errors for unsupported legacy/server commands | no hangs; UI cancellation; compatibility report lists every boundary |
| P8 Release | Distribution proof | license/notices/SBOM; full simulator/device matrix; memory/repeat/network tests | release report and zero unknown command behavior |

## Priority queue

Work proceeds without switching to unrelated command families:

1. Add differential coverage and implement `restore`, `clean`, `rev-parse`,
   `ls-files`, `ls-tree`, `show-ref`, `symbolic-ref`, `merge-base`.
2. Refactor the monolithic dispatcher behind a registry without changing
   behavior.
3. Replace environment-only credentials with the session callback interface.
4. Complete HTTPS `ls-remote` and refspec/private-auth/cancellation behavior.
5. Enable SSH and implement Keychain + host-key verification.
6. Continue P2 through P8 in table order.

## Verification matrix

Every supported command must pass:

| Dimension | Cases |
|---|---|
| Parser | short/long flags, combined flags, `--`, missing values, unknown flags |
| Result | stdout, stderr, exit status, repository/index/worktree/ref state |
| Lifecycle | clean, dirty, conflict, continue, skip, abort, detached HEAD |
| Invocation | repeated calls, pipeline, `-C`, git-dir/work-tree, spaces/non-ASCII paths |
| Sessions | parallel different repositories; serialized mutation of same repository |
| Network | public/private, redirects, TLS failure, auth retry, shallow, cancel, reconnect |
| Apple | macOS oracle, iOS simulator, signed physical device |

The existing `core/tests/git_compat.rs` remains the desktop Git oracle. Tests
must compare state and exit codes first, then add stable output assertions;
tests may not merely assert that a command did not crash.

## Explicit non-solutions

- Do not copy upstream GPL command parsers.
- Do not implement hundreds of flags as unrelated ad-hoc `if` chains.
- Do not claim `git2-rs` itself provides a CLI; it is the engine.
- Do not route Git through WASI merely to obtain a process boundary.
- Do not weaken TLS or SSH host-key validation to make a demo pass.
- Do not advertise complete Git while commands silently ignore options.

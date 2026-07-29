// Copyright 2017 Google Inc.
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::cell::RefCell;
use std::error::Error;
use std::ffi::OsString;
use std::io::{stderr, Write};
use std::path::Path;

use super::{Matcher, MatcherIO, WalkEntry};

enum Arg {
    FileArg(Vec<OsString>),
    LiteralArg(OsString),
}

fn parse_arg(s: &str) -> Arg {
    let parts = s.split("{}").collect::<Vec<_>>();
    if parts.len() == 1 {
        Arg::LiteralArg(OsString::from(s))
    } else {
        Arg::FileArg(parts.iter().map(OsString::from).collect())
    }
}

pub struct SingleExecMatcher {
    executable: Arg,
    args: Vec<Arg>,
    exec_in_parent_dir: bool,
    interactive: bool,
}

impl SingleExecMatcher {
    pub fn new(
        executable: &str,
        args: &[&str],
        exec_in_parent_dir: bool,
    ) -> Result<Self, Box<dyn Error>> {
        Ok(Self::new_impl(executable, args, exec_in_parent_dir, false))
    }

    pub fn new_interactive(
        executable: &str,
        args: &[&str],
        exec_in_parent_dir: bool,
    ) -> Result<Self, Box<dyn Error>> {
        Ok(Self::new_impl(executable, args, exec_in_parent_dir, true))
    }

    fn new_impl(
        executable: &str,
        args: &[&str],
        exec_in_parent_dir: bool,
        interactive: bool,
    ) -> Self {
        let transformed_args = args.iter().map(|&a| parse_arg(a)).collect();

        Self {
            executable: parse_arg(executable),
            args: transformed_args,
            exec_in_parent_dir,
            interactive,
        }
    }
}

impl Matcher for SingleExecMatcher {
    fn matches(&self, file_info: &WalkEntry, matcher_io: &mut MatcherIO) -> bool {
        let path_to_file = if self.exec_in_parent_dir {
            if let Some(f) = file_info.path().file_name() {
                Path::new(".").join(f)
            } else {
                Path::new(".").join(file_info.path())
            }
        } else {
            file_info.path().to_path_buf()
        };

        let resolved_executable = match self.executable {
            Arg::LiteralArg(ref a) => a.clone(),
            Arg::FileArg(ref parts) => parts.join(path_to_file.as_os_str()),
        };

        if self.interactive {
            // GNU find prints a fixed, abbreviated prompt of the form
            // "< executable ... pathname > ? ".  It does not render the
            // substituted argument list, and always shows the full path of
            // the entry being processed (even for -okdir, whose command runs
            // with the "./basename" form).
            let prompt = format!(
                "< {} ... {} > ? ",
                resolved_executable.to_string_lossy(),
                file_info.path().to_string_lossy()
            );

            if !matcher_io.confirm(&prompt) {
                return false;
            }
        }

        // YourShell patch: route through the pluggable runner (crate::exec_hook)
        // instead of std::process::Command. iOS forbids fork/exec, so the
        // embedder runs this argv in an in-process subshell; with no hook
        // installed the runner still uses std::process::Command, unchanged.
        let mut argv: Vec<OsString> = vec![resolved_executable.clone()];
        for arg in &self.args {
            match *arg {
                Arg::LiteralArg(ref a) => argv.push(a.as_os_str().to_os_string()),
                Arg::FileArg(ref parts) => argv.push(parts.join(path_to_file.as_os_str())),
            }
        }
        let mut cwd: Option<std::path::PathBuf> = None;
        if self.exec_in_parent_dir {
            match file_info.path().parent() {
                None => {
                    // Root paths like "/" have no parent.  Run them from the root to match GNU find.
                    cwd = Some(file_info.path().to_path_buf());
                }
                Some(parent) if parent == Path::new("") => {
                    // Paths like "foo" have a parent of "".  Avoid chdir("").
                }
                Some(parent) => {
                    cwd = Some(parent.to_path_buf());
                }
            }
        }
        let env: Vec<(OsString, OsString)> = std::env::vars_os().collect();
        let code = crate::exec_hook::run(&argv, &env, cwd.as_deref(), false);
        if code == 127 {
            writeln!(
                &mut stderr(),
                "Failed to run {}: command not found",
                resolved_executable.to_string_lossy()
            )
            .unwrap();
            return false;
        }
        code == 0
    }

    fn has_side_effects(&self) -> bool {
        true
    }
}

pub struct MultiExecMatcher {
    executable: String,
    args: Vec<OsString>,
    exec_in_parent_dir: bool,
    /// Command to build while matching.
    command: RefCell<Option<argmax::Command>>,
    /// YourShell patch: the same argv, mirrored so it can be handed to the
    /// pluggable runner (`crate::exec_hook`). `argmax::Command` is kept purely
    /// as the ARG_MAX gate — it wraps `std::process::Command`, which cannot run
    /// on a platform without fork/exec, and it does not expose its arguments.
    argv: RefCell<Vec<OsString>>,
    /// Working directory accumulated for `-execdir`, applied at dispatch time.
    exec_cwd: RefCell<Option<std::path::PathBuf>>,
}

impl MultiExecMatcher {
    pub fn new(
        executable: &str,
        args: &[&str],
        exec_in_parent_dir: bool,
    ) -> Result<Self, Box<dyn Error>> {
        let transformed_args = args.iter().map(OsString::from).collect();

        Ok(Self {
            executable: executable.to_string(),
            args: transformed_args,
            exec_in_parent_dir,
            command: RefCell::new(None),
            argv: RefCell::new(Vec::new()),
            exec_cwd: RefCell::new(None),
        })
    }

    fn new_command(&self) -> argmax::Command {
        let mut command = argmax::Command::new(&self.executable);
        command.try_args(&self.args).unwrap();
        // Keep the mirrored argv in step with the argmax command.
        let mut argv = self.argv.borrow_mut();
        argv.clear();
        argv.push(OsString::from(&self.executable));
        argv.extend(self.args.iter().cloned());
        command
    }

    fn run_command(&self, _command: &mut argmax::Command, matcher_io: &mut MatcherIO) {
        // YourShell patch: dispatch the mirrored argv through the pluggable
        // runner rather than `argmax::Command::status()`.
        let argv = self.argv.borrow().clone();
        let cwd = self.exec_cwd.borrow_mut().take();
        let env: Vec<(OsString, OsString)> = std::env::vars_os().collect();
        let code = crate::exec_hook::run(&argv, &env, cwd.as_deref(), false);
        if code == 127 {
            writeln!(
                &mut stderr(),
                "Failed to run {}: command not found",
                self.executable
            )
            .unwrap();
            matcher_io.set_exit_code(1);
        } else if code != 0 {
            matcher_io.set_exit_code(1);
        }
    }
}

impl Matcher for MultiExecMatcher {
    fn matches(&self, file_info: &WalkEntry, matcher_io: &mut MatcherIO) -> bool {
        let path_to_file = if self.exec_in_parent_dir {
            if let Some(f) = file_info.path().file_name() {
                Path::new(".").join(f)
            } else {
                Path::new(".").join(file_info.path())
            }
        } else {
            file_info.path().to_path_buf()
        };
        let mut command = self.command.borrow_mut();
        let command = command.get_or_insert_with(|| self.new_command());

        // Build command, or dispatch it before when it is long enough.
        // (YourShell patch: every successful `try_arg` is mirrored into `argv`,
        // which is what actually gets executed — see `run_command`.)
        if command.try_arg(&path_to_file).is_ok() {
            self.argv.borrow_mut().push(path_to_file.clone().into_os_string());
        } else {
            if self.exec_in_parent_dir {
                match file_info.path().parent() {
                    None => {
                        // Root paths like "/" have no parent.  Run them from the root to match GNU find.
                        command.current_dir(file_info.path());
                        *self.exec_cwd.borrow_mut() = Some(file_info.path().to_path_buf());
                    }
                    Some(parent) if parent == Path::new("") => {
                        // Paths like "foo" have a parent of "".  Avoid chdir("").
                    }
                    Some(parent) => {
                        command.current_dir(parent);
                        *self.exec_cwd.borrow_mut() = Some(parent.to_path_buf());
                    }
                }
            }
            self.run_command(command, matcher_io);

            // Reset command status.
            *command = self.new_command();
            if let Err(e) = command.try_arg(&path_to_file) {
                writeln!(
                    &mut stderr(),
                    "Cannot fit a single argument {}: {}",
                    &path_to_file.to_string_lossy(),
                    e
                )
                .unwrap();
                matcher_io.set_exit_code(1);
            } else {
                self.argv.borrow_mut().push(path_to_file.clone().into_os_string());
            }
        }
        true
    }

    fn finished_dir(&self, dir: &Path, matcher_io: &mut MatcherIO) {
        // Dispatch command for -execdir.
        if self.exec_in_parent_dir {
            let mut command = self.command.borrow_mut();
            if let Some(mut command) = command.take() {
                command.current_dir(Path::new(".").join(dir));
                self.run_command(&mut command, matcher_io);
            }
        }
    }

    fn finished(&self, matcher_io: &mut MatcherIO) {
        // Dispatch command for -exec.
        if !self.exec_in_parent_dir {
            let mut command = self.command.borrow_mut();
            if let Some(mut command) = command.take() {
                self.run_command(&mut command, matcher_io);
            }
        }
    }

    fn has_side_effects(&self) -> bool {
        true
    }
}

#[cfg(test)]
/// No tests here, because we need to call out to an external executable. See
/// `tests/exec_unit_tests.rs` instead.
mod tests {}

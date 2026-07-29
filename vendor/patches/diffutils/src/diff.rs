// This file is part of the uutils diffutils package.
//
// For the full copyright and license information, please view the LICENSE-*
// files that was distributed with this source code.

use crate::params::{parse_params, Format};
use crate::utils::report_failure_to_read_input_file;
use crate::{context_diff, ed_diff, normal_diff, side_diff, unified_diff};
use std::ffi::OsString;
use std::fs;
use std::io::{self, stdout, Read, Write};
use std::iter::Peekable;

// Exit codes are documented at
// https://www.gnu.org/software/diffutils/manual/html_node/Invoking-diff.html.
//     An exit status of 0 means no differences were found,
//     1 means some differences were found,
//     and 2 means trouble.
// YourShell patch: the signature was `Peekable<ArgsOs>`, and `ArgsOs` can only
// come from `std::env::args_os()` — impossible to build from a shell's argv.
// `parse_params` was already generic, so only this signature needed widening.
// YourShell patch: return a plain exit code instead of `std::process::ExitCode`.
// `ExitCode` is opaque — the host shell has to hand the number back to the
// caller's `$?`, and there is no way to read it out of an `ExitCode`.
pub fn main<I: Iterator<Item = OsString>>(opts: Peekable<I>) -> i32 {
    // YourShell patch: was `exit(2)`. This runs inside a long-lived host process
    // shared with the UI and every other command, so exiting here would take the
    // whole app down over a bad diff flag.
    let params = match parse_params(opts) {
        Ok(p) => p,
        Err(error) => {
            eprintln!("{error}");
            return 2;
        }
    };
    // if from and to are the same file, no need to perform any comparison
    let maybe_report_identical_files = || {
        if params.report_identical_files {
            println!(
                "Files {} and {} are identical",
                params.from.to_string_lossy(),
                params.to.to_string_lossy(),
            );
        }
    };
    if params.from == "-" && params.to == "-"
        || same_file::is_same_file(&params.from, &params.to).unwrap_or(false)
    {
        maybe_report_identical_files();
        return 0;
    }

    // read files
    fn read_file_contents(filepath: &OsString) -> io::Result<Vec<u8>> {
        if filepath == "-" {
            let mut content = Vec::new();
            io::stdin().read_to_end(&mut content).and(Ok(content))
        } else {
            fs::read(filepath)
        }
    }
    let mut io_error = false;
    let from_content = match read_file_contents(&params.from) {
        Ok(from_content) => from_content,
        Err(e) => {
            report_failure_to_read_input_file(&params.executable, &params.from, &e);
            io_error = true;
            vec![]
        }
    };
    let to_content = match read_file_contents(&params.to) {
        Ok(to_content) => to_content,
        Err(e) => {
            report_failure_to_read_input_file(&params.executable, &params.to, &e);
            io_error = true;
            vec![]
        }
    };
    if io_error {
        return 2;
    }

    // run diff
    let result: Vec<u8> = match params.format {
        Format::Normal => normal_diff::diff(&from_content, &to_content, &params),
        Format::Unified => unified_diff::diff(&from_content, &to_content, &params),
        Format::Context => context_diff::diff(&from_content, &to_content, &params),
        // YourShell patch: was `exit(2)` — see the note above.
        Format::Ed => match ed_diff::diff(&from_content, &to_content, &params) {
            Ok(v) => v,
            Err(error) => {
                eprintln!("{error}");
                return 2;
            }
        },
        Format::SideBySide => {
            let mut output = stdout().lock();
            side_diff::diff(&from_content, &to_content, &mut output, &params)
        }
    };
    if params.brief && !result.is_empty() {
        println!(
            "Files {} and {} differ",
            params.from.to_string_lossy(),
            params.to.to_string_lossy()
        );
    } else {
        io::stdout().write_all(&result).unwrap();
    }
    if result.is_empty() {
        maybe_report_identical_files();
        0
    } else {
        1
    }
}

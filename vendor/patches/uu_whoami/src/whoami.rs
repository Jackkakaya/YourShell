// This file is part of the uutils coreutils package.
//
// For the full copyright and license information, please view the LICENSE
// file that was distributed with this source code.

use clap::Command;
use std::ffi::OsString;
use uucore::display::println_verbatim;
use uucore::error::{FromIo, UResult};
use uucore::translate;

mod platform;

#[uucore::main(no_signals)]
pub fn uumain(args: impl uucore::Args) -> UResult<()> {
    uucore::clap_localization::handle_clap_result(uu_app(), args)?;
    let username = whoami()?;
    println_verbatim(username).map_err_context(|| translate!("whoami-error-failed-to-print"))?;
    Ok(())
}

/// Get the current username
pub fn whoami() -> UResult<OsString> {
    match platform::get_username() {
        Ok(username) => Ok(username),
        // Sandboxed platforms (e.g. iOS) have no passwd database; fall back
        // to the USER environment variable, then to the mobile user's
        // conventional name.
        Err(_e) => Ok(std::env::var_os("USER")
            .unwrap_or_else(|| OsString::from("mobile"))),
    }
}

pub fn uu_app() -> Command {
    Command::new("whoami")
        .version(uucore::crate_version!())
        .help_template(uucore::localized_help_template("whoami"))
        .about(translate!("whoami-about"))
        .override_usage(translate!("whoami-usage"))
        .infer_long_args(true)
}

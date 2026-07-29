//! wget-compatible frontend over the embedded upstream curl CLI.
//!
//! GNU Wget is not safely embeddable on iOS. HTTP, TLS, proxy, cookie,
//! redirect, retry and transfer behavior are delegated to upstream curl; this
//! module only converts common Wget option spellings.

use std::ffi::{c_char, c_int};

use brush_core::builtins::{ContentOptions, ContentType, Registration};
use brush_core::extensions::DefaultShellExtensions;
use brush_core::{CommandArg, ExecutionContext, ExecutionResult};
use futures::future::BoxFuture;

use crate::command_host;

unsafe extern "C" {
    fn ys_curl_run(argc: c_int, argv: *mut *mut c_char) -> c_int;
}

fn content(
    name: &str,
    _content_type: ContentType,
    _options: &ContentOptions,
) -> Result<String, brush_core::Error> {
    Ok(format!("{name}: wget frontend over curl 8.1.2"))
}

fn take_value(args: &[String], index: &mut usize, option: &str) -> Result<String, String> {
    *index += 1;
    args.get(*index)
        .cloned()
        .ok_or_else(|| format!("wget: option '{option}' requires an argument"))
}

fn required_value(args: &[String], index: &mut usize, option: &str) -> Option<String> {
    match take_value(args, index, option) {
        Ok(value) => Some(value),
        Err(message) => {
            eprintln!("{message}");
            None
        }
    }
}

fn wget_main_with(ctx: &command_host::CmdCtx<'_>, run_curl: impl FnOnce(&[String]) -> i32) -> i32 {
    let mut curl = vec![
        "curl".to_string(),
        "--location".to_string(),
        "--fail".to_string(),
    ];
    let mut index = 1;
    let mut explicit_output = false;
    let mut url_count = 0usize;
    let mut user: Option<String> = None;
    let mut password: Option<String> = None;
    let mut proxy_user: Option<String> = None;
    let mut proxy_password: Option<String> = None;

    while index < ctx.argv.len() {
        let arg = &ctx.argv[index];
        match arg.as_str() {
            "-O" | "--output-document" => {
                let Some(value) = required_value(ctx.argv, &mut index, arg) else {
                    return 2;
                };
                explicit_output = true;
                if value != "-" {
                    curl.extend(["--output".to_string(), value]);
                }
            }
            "-q" | "--quiet" => curl.push("--silent".to_string()),
            "-4" | "--inet4-only" => curl.push("--ipv4".to_string()),
            "-6" | "--inet6-only" => curl.push("--ipv6".to_string()),
            "-nv" | "--no-verbose" => {
                curl.extend(["--silent".to_string(), "--show-error".to_string()])
            }
            "-c" | "--continue" => curl.extend(["--continue-at".to_string(), "-".to_string()]),
            "-nc" | "--no-clobber" => curl.push("--no-clobber".to_string()),
            "--no-check-certificate" => curl.push("--insecure".to_string()),
            "--no-proxy" => curl.extend(["--noproxy".to_string(), "*".to_string()]),
            "--retry-connrefused" => curl.push("--retry-connrefused".to_string()),
            "--ignore-length" => curl.push("--ignore-content-length".to_string()),
            "--no-cache" => curl.extend([
                "--header".to_string(),
                "Cache-Control: no-cache".to_string(),
                "--header".to_string(),
                "Pragma: no-cache".to_string(),
            ]),
            "-S" | "--server-response" => {
                curl.extend(["--dump-header".to_string(), "-".to_string()])
            }
            "--content-disposition" => {
                curl.extend([
                    "--remote-header-name".to_string(),
                    "--remote-name".to_string(),
                ]);
                explicit_output = true;
            }
            "--compression=auto" => curl.push("--compressed".to_string()),
            "--spider" => {
                curl.extend([
                    "--head".to_string(),
                    "--output".to_string(),
                    "/dev/null".to_string(),
                ]);
                explicit_output = true;
            }
            "-P" | "--directory-prefix" => {
                let Some(value) = required_value(ctx.argv, &mut index, arg) else {
                    return 2;
                };
                curl.extend([
                    "--output-dir".to_string(),
                    value,
                    "--create-dirs".to_string(),
                ]);
            }
            "--header" => {
                let Some(value) = required_value(ctx.argv, &mut index, arg) else {
                    return 2;
                };
                curl.extend(["--header".to_string(), value]);
            }
            "-e" | "--referer" => {
                let Some(value) = required_value(ctx.argv, &mut index, arg) else {
                    return 2;
                };
                curl.extend(["--referer".to_string(), value]);
            }
            "-U" | "--user-agent" => {
                let Some(value) = required_value(ctx.argv, &mut index, arg) else {
                    return 2;
                };
                curl.extend(["--user-agent".to_string(), value]);
            }
            "--connect-timeout" => {
                let Some(value) = required_value(ctx.argv, &mut index, arg) else {
                    return 2;
                };
                curl.extend(["--connect-timeout".to_string(), value]);
            }
            "--max-redirect" => {
                let Some(value) = required_value(ctx.argv, &mut index, arg) else {
                    return 2;
                };
                curl.extend(["--max-redirs".to_string(), value]);
            }
            "--load-cookies" => {
                let Some(value) = required_value(ctx.argv, &mut index, arg) else {
                    return 2;
                };
                curl.extend(["--cookie".to_string(), value]);
            }
            "--save-cookies" => {
                let Some(value) = required_value(ctx.argv, &mut index, arg) else {
                    return 2;
                };
                curl.extend(["--cookie-jar".to_string(), value]);
            }
            "--keep-session-cookies" => {
                // curl has no persistent cookie engine unless --cookie or
                // --cookie-jar is supplied; its jar already retains session
                // cookies, so this Wget switch needs no extra argv.
            }
            "--user" | "--http-user" => {
                user = required_value(ctx.argv, &mut index, arg);
                if user.is_none() {
                    return 2;
                }
            }
            "--password" | "--http-password" => {
                password = required_value(ctx.argv, &mut index, arg);
                if password.is_none() {
                    return 2;
                }
            }
            "--proxy-user" => {
                proxy_user = required_value(ctx.argv, &mut index, arg);
                if proxy_user.is_none() {
                    return 2;
                }
            }
            "--proxy-password" => {
                proxy_password = required_value(ctx.argv, &mut index, arg);
                if proxy_password.is_none() {
                    return 2;
                }
            }
            "--method" => {
                let Some(value) = required_value(ctx.argv, &mut index, arg) else {
                    return 2;
                };
                curl.extend(["--request".to_string(), value]);
            }
            "--body-data" => {
                let Some(value) = required_value(ctx.argv, &mut index, arg) else {
                    return 2;
                };
                curl.extend(["--data".to_string(), value]);
            }
            "--body-file" | "--post-file" => {
                let Some(value) = required_value(ctx.argv, &mut index, arg) else {
                    return 2;
                };
                curl.extend(["--data-binary".to_string(), format!("@{value}")]);
            }
            "-T" | "--timeout" => {
                let Some(value) = required_value(ctx.argv, &mut index, arg) else {
                    return 2;
                };
                curl.extend(["--max-time".to_string(), value]);
            }
            "--waitretry" => {
                let Some(value) = required_value(ctx.argv, &mut index, arg) else {
                    return 2;
                };
                curl.extend(["--retry-delay".to_string(), value]);
            }
            "-t" | "--tries" => {
                let Some(value) = required_value(ctx.argv, &mut index, arg) else {
                    return 2;
                };
                let retries = value.parse::<u32>().unwrap_or(1).saturating_sub(1);
                curl.extend(["--retry".to_string(), retries.to_string()]);
            }
            "--post-data" => {
                let Some(value) = required_value(ctx.argv, &mut index, arg) else {
                    return 2;
                };
                curl.extend(["--data".to_string(), value]);
            }
            "--help" => {
                println!(
                    "Usage: wget [OPTION]... URL...\n\
                     HTTP/TLS transfers use embedded curl 8.1.2.\n\
                     Recursive mirroring is unavailable on iOS."
                );
                return 0;
            }
            "--version" => {
                println!("GNU Wget compatibility frontend (curl 8.1.2 backend)");
                return 0;
            }
            "-r" | "--recursive" | "-m" | "--mirror" => {
                eprintln!("wget: recursive mirroring is unavailable on iOS");
                return 2;
            }
            _ if arg.starts_with("--output-document=") => {
                explicit_output = true;
                let value = arg.trim_start_matches("--output-document=");
                if value != "-" {
                    curl.extend(["--output".to_string(), value.to_string()]);
                }
            }
            _ if arg.starts_with("--directory-prefix=") => curl.extend([
                "--output-dir".to_string(),
                arg.trim_start_matches("--directory-prefix=").to_string(),
                "--create-dirs".to_string(),
            ]),
            _ if arg.starts_with("--timeout=") => curl.extend([
                "--max-time".to_string(),
                arg.trim_start_matches("--timeout=").to_string(),
            ]),
            _ if arg.starts_with("--connect-timeout=") => curl.extend([
                "--connect-timeout".to_string(),
                arg.trim_start_matches("--connect-timeout=").to_string(),
            ]),
            _ if arg.starts_with("--max-redirect=") => curl.extend([
                "--max-redirs".to_string(),
                arg.trim_start_matches("--max-redirect=").to_string(),
            ]),
            _ if arg.starts_with("--header=") => curl.extend([
                "--header".to_string(),
                arg.trim_start_matches("--header=").to_string(),
            ]),
            _ if arg.starts_with("--referer=") => curl.extend([
                "--referer".to_string(),
                arg.trim_start_matches("--referer=").to_string(),
            ]),
            _ if arg.starts_with("--user-agent=") => curl.extend([
                "--user-agent".to_string(),
                arg.trim_start_matches("--user-agent=").to_string(),
            ]),
            _ if arg.starts_with("--load-cookies=") => curl.extend([
                "--cookie".to_string(),
                arg.trim_start_matches("--load-cookies=").to_string(),
            ]),
            _ if arg.starts_with("--save-cookies=") => curl.extend([
                "--cookie-jar".to_string(),
                arg.trim_start_matches("--save-cookies=").to_string(),
            ]),
            _ if arg.starts_with("--method=") => curl.extend([
                "--request".to_string(),
                arg.trim_start_matches("--method=").to_string(),
            ]),
            _ if arg.starts_with("--body-data=") => curl.extend([
                "--data".to_string(),
                arg.trim_start_matches("--body-data=").to_string(),
            ]),
            _ if arg.starts_with("--body-file=") || arg.starts_with("--post-file=") => {
                let value = arg
                    .split_once('=')
                    .map(|(_, value)| value)
                    .unwrap_or_default();
                curl.extend(["--data-binary".to_string(), format!("@{value}")]);
            }
            _ if arg.starts_with("--user=") || arg.starts_with("--http-user=") => {
                user = arg.split_once('=').map(|(_, value)| value.to_string());
            }
            _ if arg.starts_with("--password=") || arg.starts_with("--http-password=") => {
                password = arg.split_once('=').map(|(_, value)| value.to_string());
            }
            _ if arg.starts_with("--proxy-user=") => {
                proxy_user = arg.split_once('=').map(|(_, value)| value.to_string());
            }
            _ if arg.starts_with("--proxy-password=") => {
                proxy_password = arg.split_once('=').map(|(_, value)| value.to_string());
            }
            _ if arg.starts_with("--waitretry=") => curl.extend([
                "--retry-delay".to_string(),
                arg.trim_start_matches("--waitretry=").to_string(),
            ]),
            _ if arg.starts_with('-') => {
                eprintln!("wget: unsupported option '{arg}'");
                return 2;
            }
            _ => {
                url_count += 1;
                curl.push(arg.clone());
            }
        }
        index += 1;
    }

    if url_count == 0 {
        eprintln!("wget: missing URL");
        return 2;
    }
    if user.is_some() || password.is_some() {
        curl.extend([
            "--user".to_string(),
            format!(
                "{}:{}",
                user.unwrap_or_default(),
                password.unwrap_or_default()
            ),
        ]);
    }
    if proxy_user.is_some() || proxy_password.is_some() {
        curl.extend([
            "--proxy-user".to_string(),
            format!(
                "{}:{}",
                proxy_user.unwrap_or_default(),
                proxy_password.unwrap_or_default()
            ),
        ]);
    }
    if !explicit_output {
        curl.insert(3, "--remote-name".to_string());
    }

    run_curl(&curl)
}

fn wget_main(ctx: &command_host::CmdCtx<'_>) -> i32 {
    wget_main_with(ctx, |curl| {
        let translated = command_host::CmdCtx {
            name: "curl",
            argv: curl,
            env: ctx.env,
            cwd: ctx.cwd,
            stdin_is_interactive: ctx.stdin_is_interactive,
        };
        command_host::run_c_argv(&translated, ys_curl_run)
    })
}

fn exec(
    context: ExecutionContext<'_, DefaultShellExtensions>,
    args: Vec<CommandArg>,
) -> BoxFuture<'_, Result<ExecutionResult, brush_core::Error>> {
    command_host::dispatch(context, args, wget_main)
}

pub fn registration() -> Registration<DefaultShellExtensions> {
    Registration {
        execute_func: exec,
        content_func: content,
        disabled: false,
        special_builtin: false,
        declaration_builtin: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn invoke(args: &[&str]) -> (i32, Vec<String>) {
        let argv: Vec<String> = std::iter::once("wget")
            .chain(args.iter().copied())
            .map(str::to_string)
            .collect();
        let ctx = command_host::CmdCtx {
            name: "wget",
            argv: &argv,
            env: &[],
            cwd: Path::new("."),
            stdin_is_interactive: false,
        };
        let mut translated = Vec::new();
        let code = wget_main_with(&ctx, |curl| {
            translated = curl.to_vec();
            99
        });
        (code, translated)
    }

    #[test]
    fn immediate_and_error_actions() {
        assert_eq!(invoke(&["--help"]).0, 0);
        assert_eq!(invoke(&["--version"]).0, 0);
        assert_eq!(invoke(&[]).0, 2);
        assert_eq!(invoke(&["--unknown"]).0, 2);
        for option in ["-r", "--recursive", "-m", "--mirror"] {
            assert_eq!(invoke(&[option]).0, 2);
        }
    }

    #[test]
    fn every_value_option_rejects_a_missing_value() {
        for option in [
            "-O",
            "--output-document",
            "-P",
            "--directory-prefix",
            "--header",
            "-e",
            "--referer",
            "-U",
            "--user-agent",
            "--connect-timeout",
            "--max-redirect",
            "--load-cookies",
            "--save-cookies",
            "--user",
            "--http-user",
            "--password",
            "--http-password",
            "--proxy-user",
            "--proxy-password",
            "--method",
            "--body-data",
            "--body-file",
            "--post-file",
            "-T",
            "--timeout",
            "--waitretry",
            "-t",
            "--tries",
            "--post-data",
        ] {
            assert_eq!(invoke(&[option]).0, 2, "{option} accepted a missing value");
        }
    }

    #[test]
    fn flag_only_translation_paths_reach_curl() {
        let (code, translated) = invoke(&[
            "-q",
            "-4",
            "-6",
            "-nv",
            "-c",
            "-nc",
            "--no-check-certificate",
            "--no-proxy",
            "--retry-connrefused",
            "--ignore-length",
            "--no-cache",
            "-S",
            "--content-disposition",
            "--compression=auto",
            "--keep-session-cookies",
            "file:///definitely/missing/wget-test",
        ]);
        assert_eq!(code, 99);
        for expected in [
            "--silent",
            "--ipv4",
            "--ipv6",
            "--continue-at",
            "--no-clobber",
            "--insecure",
            "--noproxy",
            "--retry-connrefused",
            "--ignore-content-length",
            "--dump-header",
            "--remote-header-name",
            "--compressed",
        ] {
            assert!(translated.iter().any(|arg| arg == expected), "{expected}");
        }
    }

    #[test]
    fn separated_value_translation_paths_reach_curl() {
        let (code, translated) = invoke(&[
            "-O",
            "-",
            "-O",
            "download.bin",
            "-P",
            "downloads",
            "--header",
            "X-Test: yes",
            "-e",
            "https://referrer.invalid/",
            "-U",
            "YourShell-Test",
            "--connect-timeout",
            "1",
            "--max-redirect",
            "2",
            "--load-cookies",
            "cookies-in.txt",
            "--save-cookies",
            "cookies-out.txt",
            "--user",
            "alice",
            "--password",
            "secret",
            "--proxy-user",
            "proxy",
            "--proxy-password",
            "secret",
            "--method",
            "GET",
            "--body-data",
            "value",
            "-T",
            "1",
            "--waitretry",
            "0",
            "-t",
            "invalid",
            "file:///definitely/missing/wget-test",
        ]);
        assert_eq!(code, 99);
        assert!(translated
            .windows(2)
            .any(|v| v == ["--user", "alice:secret"]));
        assert!(translated
            .windows(2)
            .any(|v| v == ["--proxy-user", "proxy:secret"]));
        assert!(!translated.iter().any(|arg| arg == "--remote-name"));
    }

    #[test]
    fn equals_value_and_body_translation_paths_reach_curl() {
        let (code, translated) = invoke(&[
            "--output-document=-",
            "--output-document=download.bin",
            "--directory-prefix=downloads",
            "--timeout=1",
            "--connect-timeout=1",
            "--max-redirect=2",
            "--header=X-Test: yes",
            "--referer=https://referrer.invalid/",
            "--user-agent=YourShell-Test",
            "--load-cookies=cookies-in.txt",
            "--save-cookies=cookies-out.txt",
            "--method=POST",
            "--body-data=value",
            "--body-file=body.txt",
            "--post-file=post.txt",
            "--user=alice",
            "--password=secret",
            "--proxy-user=proxy",
            "--proxy-password=secret",
            "--waitretry=0",
            "file:///definitely/missing/wget-test",
        ]);
        assert_eq!(code, 99);
        assert!(translated.windows(2).any(|v| v == ["--request", "POST"]));
        assert!(translated
            .windows(2)
            .any(|v| v == ["--data-binary", "@post.txt"]));
    }

    #[test]
    fn spider_and_post_data_paths_reach_curl() {
        assert_eq!(
            invoke(&[
                "--spider",
                "--post-data",
                "value",
                "file:///definitely/missing/wget-test"
            ])
            .0,
            99
        );
        assert_eq!(
            invoke(&[
                "--body-file",
                "missing-body",
                "file:///definitely/missing/wget-test"
            ])
            .0,
            99
        );
        assert_eq!(
            invoke(&[
                "--post-file",
                "missing-post",
                "file:///definitely/missing/wget-test"
            ])
            .0,
            99
        );
    }
}

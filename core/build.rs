//! (nextvi C build disabled — its process-global state made repeated `vi`
//!   invocations crash in our long-lived process. `vi` uses the stable Rust
//!   modal editor instead. Kept for a future thread-localized integration.)
//!
//! one-true-awk (Kernighan's awk) IS compiled in, like SQLite/libgit2: the
//! vendored C is built into a static lib via `cc` and driven in-process by
//! `awk_adapter.rs`. The grammar/proctab were pre-generated on the host (bison
//! and maketab) and checked into vendor/awk, so no yacc/bison is needed here and
//! iOS cross-compiles cleanly.

use std::path::Path;

fn main() {
    build_curl();
    build_awk();
    build_jq();
    build_sqlite_shell();
    build_bsdtar();
    build_bsd_gzip();
    build_bsdunzip();
}

fn makefile_c_sources(makefile: &Path, variables: &[&str], base: &Path) -> Vec<std::path::PathBuf> {
    let text = std::fs::read_to_string(makefile)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", makefile.display()));
    let lines: Vec<&str> = text.lines().collect();
    let mut result = Vec::new();

    for variable in variables {
        let prefix = format!("{variable} =");
        let Some(mut index) = lines
            .iter()
            .position(|line| line.trim_start().starts_with(&prefix))
        else {
            panic!("{variable} missing from {}", makefile.display());
        };
        let mut first = true;
        loop {
            let line = lines[index];
            let body = if first {
                line.split_once('=').map_or("", |(_, rest)| rest)
            } else {
                line
            };
            for token in body.split_whitespace() {
                let token = token.trim_end_matches('\\');
                if token.ends_with(".c") {
                    result.push(base.join(token));
                }
            }
            if !line.trim_end().ends_with('\\') {
                break;
            }
            first = false;
            index += 1;
        }
    }
    result
}

/// curl's official command-line frontend and libcurl, configured for Darwin's
/// native SecureTransport backend. argv parsing and transfer semantics remain
/// entirely upstream; the Rust adapter only enters `ys_curl_run`.
fn build_curl() {
    let root = Path::new("../vendor/curl");
    let lib = root.join("lib");
    let src = root.join("src");
    let lib_sources = makefile_c_sources(
        &lib.join("Makefile.inc"),
        &[
            "LIB_CFILES",
            "LIB_VAUTH_CFILES",
            "LIB_VTLS_CFILES",
            "LIB_VQUIC_CFILES",
            "LIB_VSSH_CFILES",
        ],
        &lib,
    );
    let tool_sources = makefile_c_sources(
        &src.join("Makefile.inc"),
        &["CURLX_CFILES", "CURL_CFILES"],
        &src,
    );

    // Link order is host -> tool -> libcurl. Darwin's static linker resolves
    // archives left-to-right, and cc emits Cargo link directives in compile
    // order, so keep the dependency order explicit here.
    cc::Build::new()
        .include(root.join("include"))
        .file(root.join("curl_host.c"))
        .warnings(false)
        .flag_if_supported("-std=c11")
        .compile("curl_host");

    let mut tool = cc::Build::new();
    tool.include(root)
        .include(root.join("include"))
        .include(&lib)
        .include(&src)
        .define("HAVE_CONFIG_H", "1")
        .define("CURL_STATICLIB", "1")
        .define("main", "ys_curl_main")
        .define("exit", "ys_curl_exit")
        .warnings(false)
        .flag_if_supported("-Wno-everything")
        .flag_if_supported("-std=c11");
    for source in &tool_sources {
        tool.file(source);
        println!("cargo:rerun-if-changed={}", source.display());
    }
    tool.compile("curl_tool");

    let mut library = cc::Build::new();
    library
        .include(root)
        .include(root.join("include"))
        .include(&lib)
        .define("HAVE_CONFIG_H", "1")
        .define("BUILDING_LIBCURL", "1")
        .define("CURL_STATICLIB", "1")
        .warnings(false)
        .flag_if_supported("-Wno-everything")
        .flag_if_supported("-std=c11");
    for source in &lib_sources {
        library.file(source);
        println!("cargo:rerun-if-changed={}", source.display());
    }
    library.compile("curl_lib");
    println!("cargo:rerun-if-changed=../vendor/curl/curl_host.c");
    println!("cargo:rerun-if-changed=../vendor/curl/curl_config.h");
    println!("cargo:rustc-link-lib=z");
    if std::env::var("CARGO_CFG_TARGET_VENDOR").as_deref() == Ok("apple") {
        println!("cargo:rustc-link-lib=framework=Security");
        println!("cargo:rustc-link-lib=framework=CoreFoundation");
        println!("cargo:rustc-link-lib=framework=SystemConfiguration");
    }
}

fn build_bsdunzip() {
    let root = Path::new("../vendor/bsdunzip");
    let fe = Path::new("../vendor/bsdtar/libarchive_fe");
    let sources = [
        "bsdunzip.c",
        "cmdline.c",
        "lafe_err.c",
        "lafe_fnmatch.c",
        "lafe_getline.c",
        "passphrase.c",
    ];
    let mut build = cc::Build::new();
    build
        .include(root)
        .include(fe)
        .include("../vendor/bsdtar")
        .define("HAVE_CONFIG_H", "1")
        .define("HAVE_LIBARCHIVE", "1")
        .define("main", "ys_bsdunzip_main")
        .define("exit", "ys_bsdunzip_exit")
        .warnings(false)
        .flag_if_supported("-Wno-everything")
        .flag_if_supported("-std=c11");
    for src in sources {
        let path = if src.starts_with('l') || src == "passphrase.c" {
            fe.join(src)
        } else {
            root.join(src)
        };
        build.file(path);
        println!(
            "cargo:rerun-if-changed=../vendor/{}",
            if src.starts_with('l') || src == "passphrase.c" {
                format!("bsdtar/libarchive_fe/{src}")
            } else {
                format!("bsdunzip/{src}")
            }
        );
    }
    build.compile("bsdunzip");
    cc::Build::new()
        .file(root.join("bsdunzip_host.c"))
        .warnings(false)
        .flag_if_supported("-std=c11")
        .compile("bsdunzip_host");
    println!("cargo:rerun-if-changed=../vendor/bsdunzip/bsdunzip_host.c");
    println!("cargo:rustc-link-lib=archive");
}

/// BSD-licensed FreeBSD/NetBSD gzip frontend, using Apple's libz.
fn build_bsd_gzip() {
    let root = Path::new("../vendor/bsd-gzip");
    let mut build = cc::Build::new();
    build
        .include(root)
        .file(root.join("gzip.c"))
        .include(root)
        .define("main", "ys_bsd_gzip_main")
        .define("exit", "ys_bsd_gzip_exit")
        .define("NO_BZIP2_SUPPORT", "1")
        .define("NO_XZ_SUPPORT", "1")
        .define("NO_ZSTD_SUPPORT", "1")
        .define("NO_COMPRESS_SUPPORT", "1")
        .define("NO_PACK_SUPPORT", "1")
        .warnings(false)
        .flag_if_supported("-Wno-everything")
        .flag_if_supported("-std=c11")
        .flag("-include")
        .flag(root.join("compat.h"));
    build.compile("bsd_gzip");

    cc::Build::new()
        .file(root.join("gzip_host.c"))
        .warnings(false)
        .flag_if_supported("-std=c11")
        .compile("bsd_gzip_host");
    for file in [
        "gzip.c",
        "unbzip2.c",
        "unlz.c",
        "unpack.c",
        "unxz.c",
        "unzstd.c",
        "zuncompress.c",
        "compat.h",
        "sys/endian.h",
        "gzip_host.c",
    ] {
        println!("cargo:rerun-if-changed=../vendor/bsd-gzip/{file}");
    }
    println!("cargo:rustc-link-lib=z");
}

/// libarchive's official bsdtar CLI, linked to the public platform libarchive.
fn build_bsdtar() {
    let root = Path::new("../vendor/bsdtar");
    let sources = [
        "tar/bsdtar.c",
        "tar/cmdline.c",
        "tar/creation_set.c",
        "tar/read.c",
        "tar/subst.c",
        "tar/util.c",
        "tar/write.c",
        "libarchive_fe/lafe_err.c",
        "libarchive_fe/lafe_fnmatch.c",
        "libarchive_fe/lafe_getline.c",
        "libarchive_fe/line_reader.c",
        "libarchive_fe/passphrase.c",
        // Added in libarchive 3.8; older Apple libarchive exports do not yet
        // provide it, so keep this one public-domain helper with the frontend.
        "archive_parse_date.c",
    ];

    let mut build = cc::Build::new();
    build
        .include(root)
        .include(root.join("tar"))
        .include(root.join("libarchive_fe"))
        .define("HAVE_CONFIG_H", "1")
        .define("HAVE_LIBARCHIVE", "1")
        .define("main", "ys_bsdtar_main")
        .define("exit", "ys_bsdtar_exit")
        .warnings(false)
        .flag_if_supported("-Wno-everything")
        .flag_if_supported("-std=c11");
    for source in sources {
        build.file(root.join(source));
        println!("cargo:rerun-if-changed=../vendor/bsdtar/{source}");
    }
    build.compile("bsdtar");

    cc::Build::new()
        .file(root.join("bsdtar_host.c"))
        .warnings(false)
        .flag_if_supported("-std=c11")
        .compile("bsdtar_host");
    println!("cargo:rerun-if-changed=../vendor/bsdtar/bsdtar_host.c");
    println!("cargo:rustc-link-lib=archive");
}

/// SQLite's official CLI (`shell.c`) linked against the exact SQLite engine
/// already supplied by rusqlite/libsqlite3-sys.
fn build_sqlite_shell() {
    let sqlite_dir = Path::new("../vendor/sqlite-shell");

    let mut shell = cc::Build::new();
    shell
        .include(sqlite_dir)
        .file(sqlite_dir.join("shell.c"))
        .define("main", "ys_sqlite3_main")
        .define("exit", "ys_sqlite3_exit")
        .define("atexit", "ys_sqlite3_atexit")
        .define("SQLITE_THREADSAFE", "1")
        .define("SQLITE_ENABLE_EXPLAIN_COMMENTS", "1")
        .define("SQLITE_ENABLE_UNKNOWN_SQL_FUNCTION", "1")
        .define("SQLITE_ENABLE_STMTVTAB", "1")
        .define("SQLITE_ENABLE_DBPAGE_VTAB", "1")
        .define("SQLITE_ENABLE_DBSTAT_VTAB", "1")
        .define("SQLITE_ENABLE_OFFSET_SQL_FUNC", "1")
        .define("SQLITE_ENABLE_JSON1", "1")
        .define("SQLITE_ENABLE_RTREE", "1")
        .define("SQLITE_ENABLE_FTS4", "1")
        .define("SQLITE_ENABLE_FTS5", "1")
        .warnings(false)
        .flag_if_supported("-Wno-everything")
        .flag_if_supported("-std=c11")
        .compile("sqlite_shell");

    let mut host = cc::Build::new();
    host.file(sqlite_dir.join("sqlite_host.c"))
        .warnings(false)
        .flag_if_supported("-std=c11")
        .compile("sqlite_host");

    for file in ["shell.c", "sqlite3.h", "sqlite_host.c"] {
        println!("cargo:rerun-if-changed=../vendor/sqlite-shell/{file}");
    }
}

/// jq itself (jqlang/jq 1.8.2, MIT), same pattern as awk: vendored C built with
/// `cc`, driven in-process by `jq_adapter.rs`.
///
/// This replaces a hand-written CLI over the `jaq` crate. jq is not a command
/// with flags over a library — it is a *language*, and the parts most likely to
/// drift (`//`, path expressions, `@base64`, error propagation, `limit`/`first`
/// short-circuiting) fail silently when reimplemented: the output still looks
/// like JSON, it is just wrong. A caller writing filters from jq's own manual
/// has no way to notice.
///
/// The three files autotools generates (`builtin.inc` — jq's standard library,
/// itself written in jq; `config_opts.inc`; `version.h`) were pre-generated on
/// the host and checked into `vendor/jq/src`, so no autotools/flex/bison is
/// needed here and iOS cross-compiles cleanly. `parser.c`/`lexer.c` ship
/// pre-generated in the upstream release tarball.
fn build_jq() {
    let jq_dir = Path::new("../vendor/jq");

    // LIBJQ_SRC from jq's Makefile.am, plus the pre-generated parser/lexer and
    // the CLI. `jq_test.c` (the upstream test runner) and `inject_errors.c` (a
    // fault-injection shim) are host-only and are not vendored.
    let sources = [
        "src/builtin.c",
        "src/bytecode.c",
        "src/compile.c",
        "src/execute.c",
        "src/jv.c",
        "src/jv_alloc.c",
        "src/jv_aux.c",
        "src/jv_dtoa.c",
        "src/jv_dtoa_tsd.c",
        "src/jv_file.c",
        "src/jv_parse.c",
        "src/jv_print.c",
        "src/jv_unicode.c",
        "src/jq_test_stub.c",
        "src/lexer.c",
        "src/linker.c",
        "src/locfile.c",
        "src/main.c",
        "src/parser.c",
        "src/util.c",
        "decNumber/decContext.c",
        "decNumber/decNumber.c",
    ];

    let mut build = cc::Build::new();
    // `src/` for jq's own headers; the crate root because main.c includes the
    // generated files as `"src/version.h"`; `onig-include/` for oniguruma.h.
    build
        .include(jq_dir.join("src"))
        .include(jq_dir)
        .include(jq_dir.join("decNumber"))
        .include(jq_dir.join("onig-include"));

    for src in &sources {
        build.file(jq_dir.join(src));
        println!("cargo:rerun-if-changed=../vendor/jq/{src}");
    }

    // jq has no config.h; autoconf passes its probe results as -D. Captured
    // from the configure-generated Makefile on macOS, which is the right answer
    // for iOS too: both are Darwin, so the libc/libm surface is identical.
    // `IEEE_8087` is little-endian, correct for every target we build.
    for def in [
        "HAVE_STDIO_H",
        "HAVE_STDLIB_H",
        "HAVE_STRING_H",
        "HAVE_INTTYPES_H",
        "HAVE_STDINT_H",
        "HAVE_STRINGS_H",
        "HAVE_SYS_STAT_H",
        "HAVE_SYS_TYPES_H",
        "HAVE_UNISTD_H",
        "HAVE_WCHAR_H",
        "HAVE_DLFCN_H",
        "HAVE_MEMMEM",
        "HAVE_PTHREAD_PRIO_INHERIT",
        "HAVE_PTHREAD",
        "HAVE_ALLOCA_H",
        "HAVE_ALLOCA",
        "HAVE_ISATTY",
        "HAVE_STRPTIME",
        "HAVE_STRFTIME",
        "HAVE_SETENV",
        "HAVE_TIMEGM",
        "HAVE_GMTIME_R",
        "HAVE_GMTIME",
        "HAVE_LOCALTIME_R",
        "HAVE_LOCALTIME",
        "HAVE_GETTIMEOFDAY",
        "HAVE_TM_TM_GMT_OFF",
        "HAVE_SETLOCALE",
        "HAVE_ARC4RANDOM",
        "HAVE_PTHREAD_KEY_CREATE",
        "HAVE_PTHREAD_ONCE",
        "HAVE_ATEXIT",
        "HAVE___THREAD",
        "IEEE_8087",
        // Regex builtins (test/match/capture/scan/split/sub/gsub). The symbols
        // come from the oniguruma that `onig_sys` already links for uu_grep —
        // exact same upstream version (6.9.10), so jq's bundled copy is
        // deliberately NOT compiled: two copies would collide at link time.
        "HAVE_LIBONIG",
        // libm, all present on Darwin.
        "HAVE_ACOS",
        "HAVE_ACOSH",
        "HAVE_ASIN",
        "HAVE_ASINH",
        "HAVE_ATAN2",
        "HAVE_ATAN",
        "HAVE_ATANH",
        "HAVE_CBRT",
        "HAVE_CEIL",
        "HAVE_COPYSIGN",
        "HAVE_COS",
        "HAVE_COSH",
        "HAVE_ERF",
        "HAVE_ERFC",
        "HAVE___EXP10",
        "HAVE_EXP2",
        "HAVE_EXP",
        "HAVE_EXPM1",
        "HAVE_FABS",
        "HAVE_FDIM",
        "HAVE_FLOOR",
        "HAVE_FMA",
        "HAVE_FMAX",
        "HAVE_FMIN",
        "HAVE_FMOD",
        "HAVE_FREXP",
        "HAVE_HYPOT",
        "HAVE_J0",
        "HAVE_J1",
        "HAVE_JN",
        "HAVE_LDEXP",
        "HAVE_LGAMMA",
        "HAVE_LOG10",
        "HAVE_LOG1P",
        "HAVE_LOG2",
        "HAVE_LOG",
        "HAVE_LOGB",
        "HAVE_MODF",
        "HAVE_LGAMMA_R",
        "HAVE_NEARBYINT",
        "HAVE_NEXTAFTER",
        "HAVE_NEXTTOWARD",
        "HAVE_POW",
        "HAVE_REMAINDER",
        "HAVE_RINT",
        "HAVE_ROUND",
        "HAVE_SCALB",
        "HAVE_SCALBLN",
        "HAVE_SCALBN",
        "HAVE_ILOGB",
        "HAVE_SIN",
        "HAVE_SINH",
        "HAVE_SQRT",
        "HAVE_TAN",
        "HAVE_TANH",
        "HAVE_TGAMMA",
        "HAVE_TRUNC",
        "HAVE_Y0",
        "HAVE_Y1",
        "HAVE_YN",
    ] {
        build.define(def, "1");
    }
    build.define("JQ_VERSION", "\"1.8.2\"");
    // Vendored code we do not want to churn; silence its warnings rather than
    // patch each one, as with awk.
    build
        .warnings(false)
        .flag_if_supported("-Wno-everything")
        .flag_if_supported("-std=c11");

    build.compile("jq");
}

fn build_awk() {
    let awk_dir = Path::new("../vendor/awk");

    // Top-level translation units from one-true-awk's makefile OFILES, plus the
    // pre-generated grammar (awkgram.tab.c) and dispatch table (proctab.c).
    // maketab.c is a host-only code generator and is intentionally excluded.
    let sources = [
        "awkgram.tab.c",
        "b.c",
        "lex.c",
        "lib.c",
        "main.c",
        "parse.c",
        "proctab.c",
        "run.c",
        "tran.c",
    ];

    let mut build = cc::Build::new();
    build.include(awk_dir);
    for src in &sources {
        build.file(awk_dir.join(src));
        println!("cargo:rerun-if-changed=../vendor/awk/{src}");
    }
    println!("cargo:rerun-if-changed=../vendor/awk/awk.h");
    println!("cargo:rerun-if-changed=../vendor/awk/proto.h");
    println!("cargo:rerun-if-changed=../vendor/awk/awkgram.tab.h");

    // one-true-awk is warning-heavy under -Wall/-pedantic; it's vendored code we
    // don't want to churn, so silence warnings rather than patch each one.
    build
        .warnings(false)
        .flag_if_supported("-Wno-everything")
        .flag_if_supported("-std=c11");

    build.compile("otawk");
}

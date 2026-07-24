//! (nextvi C build disabled — its process-global state made repeated `vi`
//! invocations crash in our long-lived process. `vi` uses the stable Rust
//! modal editor instead. Kept for a future thread-localized integration.)
//!
//! one-true-awk (Kernighan's awk) IS compiled in, like SQLite/libgit2: the
//! vendored C is built into a static lib via `cc` and driven in-process by
//! `awk_adapter.rs`. The grammar/proctab were pre-generated on the host (bison
//! + maketab) and checked into vendor/awk, so no yacc/bison is needed here and
//! iOS cross-compiles cleanly.

use std::path::Path;

fn main() {
    build_awk();
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

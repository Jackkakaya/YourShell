//! Compiles the vendored nextvi (ISC) as a static object. It's a unity build:
//! vi.c #includes the other translation units. We call its renamed entry
//! `ys_nextvi_main` from the `vi` command adapter.

fn main() {
    let vendor = "../vendor/nextvi";
    println!("cargo:rerun-if-changed={vendor}/vi.c");
    println!("cargo:rerun-if-changed={vendor}/term.c");
    cc::Build::new()
        .file(format!("{vendor}/vi.c"))
        .include(vendor)
        // nextvi expects these; harmless to define explicitly.
        .flag_if_supported("-Wno-unused-parameter")
        .flag_if_supported("-Wno-unused-result")
        .flag_if_supported("-Wno-sign-compare")
        .flag_if_supported("-Wno-implicit-fallthrough")
        .warnings(false)
        .compile("nextvi");
}

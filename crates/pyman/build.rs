//! Build script for the `pyman` GUI binary.
//!
//! Single job:
//!
//! **Windows subsystem (release only)**: tell the linker to use the `windows`
//! subsystem instead of `console`, so double-clicking / launching the GUI does
//! not open a background terminal window. We keep the standard CRT entry point
//! so the normal `fn main()` still runs and `--self-test` output works when run
//! from an existing terminal. Debug builds keep the console subsystem so
//! `eprintln!` / panic backtraces are visible.
//!
//! (There used to be a second job that embedded a separate `pyman-worker`
//! binary into this exe at build time, but that's gone now: the GUI spawns the
//! real `python` interpreter as a child and embeds no CPython itself, so there
//! is no worker binary to embed and no pyo3 to link.)

fn main() {
    set_windows_subsystem();
}

fn set_windows_subsystem() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("PROFILE").as_deref() == Ok("release")
    {
        println!("cargo:rustc-link-arg=/SUBSYSTEM:WINDOWS");
        println!("cargo:rustc-link-arg=/ENTRY:mainCRTStartup");
    }
}

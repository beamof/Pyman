//! Build script for the `pyman` GUI binary.
//!
//! On Windows (release), tell the linker to use the `windows` subsystem
//! instead of `console`, so double-clicking / launching the GUI does not open
//! a background terminal window. We keep the standard CRT entry point so the
//! normal `fn main()` still runs and `--self-test` output works when run from
//! an existing terminal.
//!
//! In debug builds we keep the console subsystem so you can see `eprintln!`
//! / panic backtraces without extra setup.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("PROFILE").as_deref() == Ok("release")
    {
        println!("cargo:rustc-link-arg=/SUBSYSTEM:WINDOWS");
        println!("cargo:rustc-link-arg=/ENTRY:mainCRTStartup");
    }
}

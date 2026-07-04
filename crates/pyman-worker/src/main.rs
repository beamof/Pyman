//! Entry point for the `pyman-worker` binary.
//!
//! Thin wrapper around [`pyman_worker::run`] — all the real logic (embedding
//! CPython, running the script, setting `sys.argv`) lives in the library so it
//! stays unit-testable. This binary is what the GUI supervisor spawns, one
//! process per running script; it is also embeddable into the GUI exe at build
//! time (see `crates/pyman/build.rs`) and extracted at runtime
//! (see `pyman::embed`).

fn main() {
    std::process::exit(pyman_worker::run());
}

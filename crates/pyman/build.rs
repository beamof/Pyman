//! Build script for the `pyman` GUI binary.
//!
//! Two jobs:
//!
//! 1. **Embed the worker binary into the GUI exe** so distribution stays a
//!    single file. The worker (`crates/pyman-worker`, the only place pyo3 is
//!    linked) is a *separate* binary precisely so the GUI's loader never has
//!    to resolve `python3.dll` at GUI startup. But we don't want users to
//!    download two files, so at build time we read the just-compiled
//!    `pyman-worker[.exe]` off disk and pass its path to the GUI via a
//!    `cargo:rustc-env`; `embed.rs` then `include_bytes!`s it into the GUI
//!    binary. The GUI extracts it to the user's data dir on first run.
//!
//!    **Build ordering — the subtle part.** Cargo guarantees a dependency's
//!    *library* is built before this script runs, but NOT its *binary*
//!    targets. `pyman-worker` ships both, and its bin (which links the large
//!    pyo3 crate) is typically still compiling when this script runs in a
//!    parallel `cargo build`. Stable Cargo has no "depend on a binary"
//!    feature (the `artifact = "bin"` syntax needs the nightly `-Z bindeps`
//!    feature). So we build the worker ourselves, here, into a **separate
//!    target directory** (`<main-target>/worker-build`). Using a separate dir
//!    is essential: a recursive `cargo build` into the *same* target dir
//!    deadlocks on the outer cargo's lock. The separate dir is a cache like
//!    any other — incremental, so it only does work when the worker changes.
//!
//! 2. **Windows subsystem (release only)**: tell the linker to use the
//!    `windows` subsystem instead of `console`, so double-clicking / launching
//!    the GUI does not open a background terminal window. We keep the standard
//!    CRT entry point so the normal `fn main()` still runs and `--self-test`
//!    output works when run from an existing terminal. Debug builds keep the
//!    console subsystem so `eprintln!` / panic backtraces are visible.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    embed_worker_binary();
    set_windows_subsystem();
}

fn embed_worker_binary() {
    // Locate the workspace root (this crate is at <root>/crates/pyman).
    // CARGO_MANIFEST_DIR = .../crates/pyman.
    let manifest_dir = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR set by Cargo"),
    );
    let workspace_root = manifest_dir
        .ancestors()
        .nth(2)
        .expect("crate is at <workspace>/crates/pyman");

    // The main build's target dir: three levels above OUT_DIR
    // (OUT_DIR = <target>/<profile>/build/<crate>-<hash>/out).
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR set by Cargo"));
    let main_target = out_dir
        .ancestors()
        .nth(3)
        .expect("OUT_DIR has at least 3 ancestors");

    // Build the worker into a *separate* target dir to avoid contending for the
    // outer cargo's lock on `main_target`. Build the same profile the GUI is
    // building so a release GUI gets a release worker.
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "debug".into());
    let worker_target = main_target.join("worker-build");

    // Build just the worker package from the workspace root (so workspace
    // deps like serde_json resolve). --locked keeps it reproducible in CI.
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut cmd = Command::new(&cargo);
    cmd.arg("build")
        .arg("--manifest-path")
        .arg(workspace_root.join("Cargo.toml"))
        .arg("-p")
        .arg("pyman-worker")
        .arg("--target-dir")
        .arg(&worker_target);
    if profile == "release" {
        cmd.arg("--release");
    }

    let status = cmd
        .status()
        .unwrap_or_else(|e| panic!("build.rs: failed to invoke cargo for worker: {e}"));
    if !status.success() {
        panic!(
            "build.rs: building pyman-worker failed (status {status}). \
             Fix the worker crate errors above, then rebuild."
        );
    }

    let exe_name = format!("pyman-worker{}", std::env::consts::EXE_SUFFIX);
    let worker_path = worker_target.join(&profile).join(&exe_name);
    if !worker_path.exists() {
        panic!(
            "build.rs: worker built but expected binary not found at {}. \
             (profile={})",
            worker_path.display(),
            profile
        );
    }

    // Hand the path to the GUI source so `embed.rs` can
    // `include_bytes!(env!("PYMAN_WORKER_BIN"))`.
    println!(
        "cargo:rustc-env=PYMAN_WORKER_BIN={}",
        worker_path.display()
    );
    // Re-run this script (and thus re-check the worker) whenever the worker's
    // sources change. Tracking the whole crate dir is coarse but correct.
    println!(
        "cargo:rerun-if-changed={}",
        workspace_root.join("crates/pyman-worker").display()
    );
}

fn set_windows_subsystem() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var("PROFILE").as_deref() == Ok("release")
    {
        println!("cargo:rustc-link-arg=/SUBSYSTEM:WINDOWS");
        println!("cargo:rustc-link-arg=/ENTRY:mainCRTStartup");
    }
}

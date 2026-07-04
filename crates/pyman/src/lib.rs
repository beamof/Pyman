//! Library facade for pyman (the GUI crate).
//!
//! PyMan is split across two crates so the GUI never links CPython:
//!   * `pyman` (this crate) — the egui GUI + supervisor. No pyo3, no
//!     `python3.dll` import, so it starts on machines without Python.
//!   * `pyman-worker` — the only place pyo3 is linked; its compiled binary is
//!     embedded into this crate's exe at build time (see `build.rs` + `embed`)
//!     and spawned per running script.
//!
//! The binary entry point (`main.rs`) runs the GUI. The non-UI modules here
//! are `pub` so integration tests (and future tooling) can use them directly.

pub mod app;
pub mod embed;
pub mod font;
pub mod history;
pub mod icon;
pub mod supervisor;
pub mod worker;

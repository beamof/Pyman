//! Library facade for pyman (the GUI crate).
//!
//! PyMan is a single crate: the egui GUI plus a supervisor that spawns the
//! real `python` interpreter as a child process per running script. The GUI
//! never embeds CPython — it links no pyo3 and carries no `python3.dll`
//! load-time dependency, so it starts on machines without Python. Both run
//! modes (script file `python <script> <args>` and CLI `python <args>`) spawn
//! the interpreter the same way; the supervisor captures each child's
//! stdout/stderr into a log buffer and polls its exit status.
//!
//! The binary entry point (`main.rs`) runs the GUI. The non-UI modules here
//! are `pub` so integration tests (and future tooling) can use them directly.

pub mod app;
pub mod font;
pub mod history;
pub mod icon;
pub mod supervisor;
pub mod worker;

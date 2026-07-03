//! Library facade for pyman.
//!
//! The binary entry point (`main.rs`) is a thin dispatcher: it either runs the
//! egui GUI (default) or, when invoked with `--worker` (or as `pyman-worker`),
//! takes the script-runner role implemented in the [`worker`] module. The
//! non-UI modules here are also `pub` so integration tests (and future
//! tooling) can use them directly.

pub mod app;
pub mod font;
pub mod history;
pub mod icon;
pub mod supervisor;
pub mod worker;

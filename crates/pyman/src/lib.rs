//! Library facade for pyman. The actual GUI lives in `main.rs` (binary); this
//! crate is also a library so integration tests (and future tooling) can use
//! the non-UI modules like `history` and `supervisor` directly.

pub mod app;
pub mod font;
pub mod history;
pub mod icon;
pub mod supervisor;

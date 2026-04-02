//! Standalone wasm-buildable consensus core.
//!
//! This crate is initially a copied slice of the existing application-side
//! consensus modules so the logic can be compiled and verified independently.

pub mod engine;
pub mod entry_buffer;
pub mod format;
pub mod llm_turn;
pub mod reducer;
pub mod render;
pub mod response;
pub mod solver;
pub mod status;
pub mod tools;
pub mod types;

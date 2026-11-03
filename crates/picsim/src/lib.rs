//! Simulator of the PIC parts Basil supports. See `docs/SIMULATOR-PLAN.md`.

pub mod config;
pub mod disasm;
pub mod hex;
pub mod machine;
pub mod mem;
pub mod periph;
pub mod pic18;
pub mod stimulus;

pub use machine::{Machine, Stop};

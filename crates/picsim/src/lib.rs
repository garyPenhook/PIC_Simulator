//! Simulator of the PIC parts Basil supports. See `docs/SIMULATOR-PLAN.md`.

pub mod disasm;
pub mod hex;
pub mod machine;
pub mod mem;
pub mod pic18;

pub use machine::{Machine, Stop};

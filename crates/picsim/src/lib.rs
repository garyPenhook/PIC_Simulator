//! Simulator of the PIC parts Basil supports. See `docs/SIMULATOR-PLAN.md`.

pub mod config;
pub mod debuginfo;
pub mod disasm;
pub mod drive;
pub mod hex;
pub mod lines;
pub mod log;
pub mod machine;
pub mod machine16;
pub mod mem;
pub mod pace;
pub mod periph;
pub mod pic16;
pub mod pic18;
pub mod stimulus;

pub use machine::{LineStep, Machine, Stop};

//! Simulator of the PIC parts Basil supports. See `docs/SIMULATOR-PLAN.md`.

mod dma;
pub mod config;
mod oscillator;
pub mod cofinfo;
pub mod debuginfo;
pub mod debugger;
pub mod disasm;
pub mod drive;
pub mod hex;
pub mod lines;
pub mod log;
pub mod machine;
pub mod machine16;
pub mod machine_ds;
pub mod mem;
pub mod pace;
pub mod periph;
pub mod dspic;
pub mod dspic_adc;
pub mod dspic_clock;
pub mod dspic_dsp;
pub mod dspic_exc;
pub mod dspic_exec;
pub mod dspic_isa;
pub mod dspic_periph;
pub mod dspic_uart;
pub mod pic16;
pub mod pic18;
pub mod stimulus;

pub use machine::{LineStep, Machine, Stop};

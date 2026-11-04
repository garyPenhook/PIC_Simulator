//! Configurable logic cells CLC1-CLC8 of the PIC18F27Q10 (DS40002043F ch.26 p.406-426).
//!
//! Rules relied on:
//! * CLCxCON (p.416): EN bit 7, OUT bit 5 (read only), INTP bit 4, INTN bit 3, MODE bits 2:0. CLCxPOL (p.417): POL bit 7, G4POL..G1POL
//!   bits 3:0. CLCxSELy (p.418-421): DyS bits 5:0. CLCxGLSy (p.422-425): bit 2(k-1)+1 = GyDkT, bit 2(k-1) = GyDkN (k = data input 1-4).
//!   CLCDATA (p.426): MLCxOUT bit x-1. With EN = 0 the cell has a logic zero output (p.416). A reset clears CLCxCON only (26.4 p.412).
//! * Gate (Figure 26-2 p.408, Table 26-1 p.410): lcxgy = (OR over the enabled terms dkT / dkN) XOR GyPOL. No enabled term gives GyPOL.
//! * Logic functions (Figure 26-3 p.411), g1..g4 = lcxg1..lcxg4:
//!   000 AND-OR (g1 & g2) | (g3 & g4); 001 OR-XOR (g1 | g2) ^ (g3 | g4); 010 AND g1 & g2 & g3 & g4; 011 S-R latch S = g1 | g2, R = g3 | g4;
//!   100 D flip-flop with S and R: D = g2, clock = g1 (rising), S = g4, R = g3; 101 D flip-flop with R: D = g4 | g2, clock g1, R = g3;
//!   110 J-K flip-flop with R: J = g2, K = g4, clock g1, R = g3; 111 transparent latch with S and R: D = g2, LE = g3, S = g4, R = g1.
//! * The output is the logic result XOR POL; a rising edge sets CLCxIF when INTP = 1, a falling edge when INTN = 1 (26.2 p.411).
//! * Output PPS codes CLC1OUT..CLC8OUT = 0x18..0x1F (Table 18-2 p.287); input codes in DS table "CLC Data Input Sources" p.409 (see `Periph::clc_input`).
//!
//! Assumptions `[A#]`:
//! * `[A83]` The cell is evaluated at the end of every instruction and at every event that changes one of its modelled inputs; it has no
//!   propagation delay (the data sheet gives it only in 39.4.20 p.715). The text of 26.1.2 says the data are "ANDed" in the gate; the figure
//!   and Table 26-1 show the enable AND per term and the OR across terms, which is what is modelled.
//! * `[A84]` In the latch and flip-flop modes the data sheet does not say which of R and S wins when both are 1: reset wins.
//! * `[A85]` Disabling the cell clears the flip-flop; enabling it takes the present clock level as the previous one (no edge).
//! * `[A86]` Modelled inputs: the CLCIN0-7 pins (CLCINxPPS), CCP1/CCP2, PWM3/PWM4, CMP1/CMP2, CWG1A/CWG1B and the CLC outputs. The clocks,
//!   timer overflows, EUSART/SPI lines, DSM, ZCD and IOC flag read low (note when a gate uses one).
//! * `[A87]` Loops through combinational cells are iterated at most 8 times per instant; a ring that has not settled then is left as it is.

use super::Flag;
use devdesc::Device;

pub struct Clc {
    pub n: u8,
    pub con: usize,
    pub pol: usize,
    pub sel: [usize; 4],
    pub gls: [usize; 4],
    pub flag: Flag,
    /// PPS output code: CLC1OUT = 0x18 .. CLC8OUT = 0x1F.
    pub sig: u8,
    /// Output after POL.
    pub out: bool,
    /// Flip-flop / latch state before POL.
    pub q: bool,
    /// Level of lcxg1 at the last evaluation (clock of the flip-flops).
    pub clk: bool,
    /// The cell was enabled at the last evaluation.
    pub on: bool,
}

impl Clc {
    pub fn find(dev: &Device, n: u8) -> Option<Clc> {
        let a = |s: &str| dev.find_sfr(&format!("CLC{n}{s}")).map(|x| x.addr as usize);
        Some(Clc {
            n,
            con: a("CON")?,
            pol: a("POL")?,
            sel: [a("SEL0")?, a("SEL1")?, a("SEL2")?, a("SEL3")?],
            gls: [a("GLS0")?, a("GLS1")?, a("GLS2")?, a("GLS3")?],
            flag: Flag::find(dev, &format!("CLC{n}IF"))?,
            sig: 0x17 + n,
            out: false,
            q: false,
            clk: false,
            on: false,
        })
    }
}

/// Gate output for the GLS register `gls`, the polarity bit `pol` and the four selected data inputs `d`.
pub fn gate(gls: u8, pol: bool, d: [bool; 4]) -> bool {
    let mut any = false;
    for (k, &v) in d.iter().enumerate() {
        any |= gls >> (2 * k + 1) & 1 != 0 && v || gls >> (2 * k) & 1 != 0 && !v;
    }
    any != pol
}

/// Next logic value (before POL) of mode `mode` for gate outputs `g` (g1..g4), the stored state `q` and the previous clock level.
pub fn logic(mode: u8, g: [bool; 4], q: bool, clk: bool) -> bool {
    let rise = g[0] && !clk;
    match mode & 7 {
        0 => g[0] && g[1] || g[2] && g[3],
        1 => (g[0] || g[1]) != (g[2] || g[3]),
        2 => g[0] && g[1] && g[2] && g[3],
        3 => {
            let (s, r) = (g[0] || g[1], g[2] || g[3]);
            if r { false } else { s || q }
        }
        4 => {
            if g[2] {
                false
            } else if g[3] {
                true
            } else if rise {
                g[1]
            } else {
                q
            }
        }
        5 => {
            if g[2] {
                false
            } else if rise {
                g[3] || g[1]
            } else {
                q
            }
        }
        6 => {
            if g[2] {
                false
            } else if rise {
                match (g[1], g[3]) {
                    (false, false) => q,
                    (true, false) => true,
                    (false, true) => false,
                    (true, true) => !q,
                }
            } else {
                q
            }
        }
        _ => {
            if g[0] {
                false
            } else if g[3] {
                true
            } else if g[2] {
                g[1]
            } else {
                q
            }
        }
    }
}

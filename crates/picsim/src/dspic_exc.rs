//! Exceptions of the dsPIC33 core: the interrupt controller's choice of the next interrupt, trap and interrupt entry,
//! and Sleep / Idle.

use crate::dspic::*;

/// Interrupt flag, enable and priority registers (IFSx at 0x800, IECx at 0x820, IPCx at 0x840): IRQ `n` is bit `n % 16`
/// of IFS`n / 16`, with its priority in the nibble `(n % 4) * 4` of IPC`n / 4`.
pub const IFS_BASE: usize = 0x800;
pub const IEC_BASE: usize = 0x820;
pub const IPC_BASE: usize = 0x840;
pub const INTCON1: usize = 0x8C0;
pub const INTCON2: usize = 0x8C2;
pub const INTTREG: usize = 0x8C8;
/// IRQ numbers go up to the last IFS register the part has (IFS11 at 0x816 -> IRQ 191)
pub const IRQ_MAX: usize = 192;

impl Dspic {
    fn mem16(&self, a: usize) -> u16 {
        u16::from(self.mem[a]) | u16::from(self.mem[a + 1]) << 8
    }

    /// The highest priority pending interrupt that the CPU would take now: `(irq, priority)`.
    pub fn irq_ready(&self) -> Option<(usize, u8)> {
        if self.mem16(INTCON2) & 0x8000 == 0 {
            return None; // GIE
        }
        let best = self.irq_best()?;
        let ipl = self.ipl();
        // DISI blocks priorities 1..6
        if self.disicnt > 0 && best.1 < 7 {
            return None;
        }
        if best.1 > ipl { Some(best) } else { None }
    }

    /// The highest priority pending (flag and enable set) interrupt with a non-zero priority.
    pub fn irq_best(&self) -> Option<(usize, u8)> {
        let mut best: Option<(usize, u8)> = None;
        for r in 0..(IRQ_MAX / 16) {
            let f = self.mem16(IFS_BASE + 2 * r);
            let e = self.mem16(IEC_BASE + 2 * r);
            let mut pend = f & e;
            if pend == 0 {
                continue;
            }
            while pend != 0 {
                let bit = pend.trailing_zeros() as usize;
                pend &= pend - 1;
                let irq = r * 16 + bit;
                let ipc = self.mem16(IPC_BASE + 2 * (irq / 4));
                let pri = ((ipc >> ((irq % 4) * 4)) & 7) as u8;
                if pri == 0 {
                    continue;
                }
                if best.is_none_or(|(_, p)| pri > p) {
                    best = Some((irq, pri));
                }
            }
        }
        best
    }

    /// Any interrupt with its flag and enable set, whatever its priority: what wakes the part from Sleep or Idle.
    fn irq_wake(&self) -> bool {
        (0..IRQ_MAX / 16).any(|r| self.mem16(IFS_BASE + 2 * r) & self.mem16(IEC_BASE + 2 * r) != 0)
    }

    /// Takes a pending trap or interrupt. Returns true when it did (the entry took its cycles).
    pub fn take_exception(&mut self) -> bool {
        if let Some(t) = self.trap_pending.take() {
            self.enter(t.vector(), 15, t.number() + 0, 4);
            return true;
        }
        if let Some((irq, pri)) = self.irq_ready() {
            let vec = 0x14 + 2 * irq as u32;
            self.enter(vec, pri, irq as u8 + 8, 4);
            return true;
        }
        false
    }

    /// Stacks the PC and `SRL`/IPL3, raises the priority level and loads the PC from the vector table.
    fn enter(&mut self, vector: u32, new_ipl: u8, number: u8, cycles: u64) {
        let pc = self.pc;
        let sr_low = (self.sr_read() & 0xFF) as u16;
        let lo = (pc as u16 & 0xFFFE) | u16::from(self.sfa);
        let ipl3 = u16::from(self.corcon & CC_IPL3 != 0);
        let hi = (sr_low << 8) | (ipl3 << 7) | ((pc >> 16) & 0x7F) as u16;
        self.push_raw(lo);
        self.push_raw(hi);
        self.sfa = false;
        self.ra = false;
        self.set_ipl(new_ipl);
        let target = self.fetch(vector) & 0x7F_FFFE;
        self.pc = target;
        self.cycles += cycles;
        // INTTREG: vector number and level
        let v = u16::from(number) | (u16::from(new_ipl & 15) << 8);
        self.mem[INTTREG] = v as u8;
        self.mem[INTTREG + 1] = (v >> 8) as u8;
        self.sleeping = None;
        self.call_depth += 1;
        self.context_enter(new_ipl);
    }

    fn push_raw(&mut self, v: u16) {
        let sp = self.w[15];
        self.mem_write16(sp, v);
        self.w[15] = sp.wrapping_add(2);
    }

    fn mem_write16(&mut self, a: u16, v: u16) {
        // the stack is always in RAM; a stack in the SFR space is caught by the stack error trap
        self.wr16(a, v);
    }

    pub(crate) fn context_enter(&mut self, _ipl: u8) {}

    pub(crate) fn on_pwrsav(&mut self, k: u8) {
        // RCON: WDTO, SLEEP, IDLE
        let rcon = 0xF80;
        self.mem[rcon] &= !0x1C;
        self.mem[rcon] |= if k == 0 { 0x08 } else { 0x04 };
    }

    pub(crate) fn check_wake(&mut self) {
        if self.sleeping.is_none() {
            return;
        }
        if self.irq_wake() || self.trap_pending.is_some() || self.p.wdt_woke() {
            self.sleeping = None;
        }
    }

    /// True when the part sleeps and nothing can wake it (no enabled interrupt, no watchdog).
    pub fn sleep_dead(&self) -> bool {
        self.sleeping.is_some() && !self.irq_wake() && !self.p.can_wake()
    }
}

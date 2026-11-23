//! RVV control state and vector length configuration.

const VLEN: u64 = 512;
const ELEN: u64 = 64;
const REGISTER_BYTES: usize = (VLEN / 8) as usize;
const VILL: u64 = 1 << 63;

#[derive(Clone)]
pub(crate) struct VectorState {
    start: u64,
    control: u64,
    length: u64,
    kind: u64,
    registers: [[u8; REGISTER_BYTES]; 32],
}

impl Default for VectorState {
    fn default() -> Self {
        Self {
            start: 0,
            control: 0,
            length: 0,
            kind: VILL,
            registers: [[0; REGISTER_BYTES]; 32],
        }
    }
}

impl VectorState {
    pub(crate) fn read(&self, csr: u32) -> Option<u64> {
        Some(match csr {
            0x008 => self.start,
            0x009 => self.control & 1,
            0x00a => self.control >> 1,
            0x00f => self.control,
            0xc20 => self.length,
            0xc21 => self.kind,
            0xc22 => VLEN / 8,
            _ => return None,
        })
    }

    pub(crate) fn write(&mut self, csr: u32, value: u64) -> bool {
        match csr {
            0x008 => self.start = value & (VLEN - 1),
            0x009 => self.control = (self.control & 6) | (value & 1),
            0x00a => self.control = (self.control & 1) | ((value & 3) << 1),
            0x00f => self.control = value & 7,
            _ => return false,
        }
        true
    }

    fn max_length(kind: u64) -> Option<u64> {
        let sew = 8 << ((kind >> 3) & 7);
        let (numerator, denominator) = match kind & 7 {
            lmul @ 0..=3 => (1 << lmul, 1),
            lmul @ 5..=7 => (1, 1 << (8 - lmul)),
            _ => return None,
        };
        if kind >> 8 != 0 || sew > ELEN / denominator {
            return None;
        }
        Some(VLEN * numerator / (sew * denominator))
    }

    pub(crate) fn configure(&mut self, kind: u64, avl: Option<u64>) -> u64 {
        let max = Self::max_length(kind);
        self.kind = if max.is_some() { kind } else { VILL };
        self.length = avl.unwrap_or(self.length).min(max.unwrap_or(0));
        self.start = 0;
        self.length
    }

    fn group_is_valid(&self, register: usize) -> bool {
        let group = 1 << (self.kind as usize & 7).min(4);
        self.kind & 7 >= 5 || register % group == 0
    }

    fn element(&self, register: usize, lane: usize, width: usize) -> i128 {
        let offset = lane * width;
        let start = offset % REGISTER_BYTES;
        let register = register + offset / REGISTER_BYTES;
        let mut bytes = [0; 8];
        bytes[..width].copy_from_slice(&self.registers[register][start..start + width]);
        let shift = 64 - width * 8;
        i128::from((u64::from_le_bytes(bytes) << shift) as i64 >> shift)
    }

    fn set_element(&mut self, register: usize, lane: usize, width: usize, value: u64) {
        let offset = lane * width;
        let start = offset % REGISTER_BYTES;
        let register = register + offset / REGISTER_BYTES;
        self.registers[register][start..start + width]
            .copy_from_slice(&value.to_le_bytes()[..width]);
    }

    pub(crate) fn execute(&mut self, instruction: u32, scalar: u64) -> Option<()> {
        if self.kind == VILL {
            return None;
        }
        let rd = ((instruction >> 7) & 31) as usize;
        let rs1 = ((instruction >> 15) & 31) as usize;
        let rs2 = ((instruction >> 20) & 31) as usize;
        let unmasked = instruction & (1 << 25) != 0;
        let width = 1 << ((self.kind >> 3) & 7);
        if !self.group_is_valid(rd) {
            return None;
        }
        match (instruction >> 26, (instruction >> 12) & 7) {
            (0x17, 4) if unmasked && rs2 == 0 => {
                for lane in self.start..self.length {
                    self.set_element(rd, lane as usize, width, scalar);
                }
            }
            (0x21, 0)
                if (unmasked || rd != 0)
                    && self.group_is_valid(rs1)
                    && self.group_is_valid(rs2) =>
            {
                let bound = 1i128 << (width * 8 - 1);
                for lane in self.start as usize..self.length as usize {
                    if !unmasked && self.registers[0][lane / 8] & (1 << (lane % 8)) == 0 {
                        continue;
                    }
                    let sum = self.element(rs1, lane, width) + self.element(rs2, lane, width);
                    let result = sum.clamp(-bound, bound - 1);
                    self.control |= u64::from(result != sum);
                    self.set_element(rd, lane, width, result as u64);
                }
            }
            _ => return None,
        }
        self.start = 0;
        Some(())
    }
}

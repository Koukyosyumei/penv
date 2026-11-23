// Configuration and CSR subset for the pinned Spike ACT profile (VLEN=1024, ELEN=64).
const VLEN: u32 = 1024;
const VILL: u32 = 1 << 31;

#[derive(Clone)]
pub struct VectorBootstrap {
    start: u32,
    control: u32,
    length: u32,
    kind: u32,
}

impl Default for VectorBootstrap {
    fn default() -> Self {
        Self {
            start: 0,
            control: 0,
            length: 0,
            kind: VILL,
        }
    }
}

impl VectorBootstrap {
    pub fn read(&self, address: u32) -> Option<u32> {
        Some(match address {
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

    pub fn write(&mut self, address: u32, value: u32) -> bool {
        match address {
            0x008 => self.start = value & (VLEN - 1),
            0x009 => self.control = (self.control & !1) | (value & 1),
            0x00a => self.control = (self.control & 1) | ((value & 3) << 1),
            0x00f => self.control = value & 7,
            _ => return false,
        }
        true
    }

    pub fn configure(&mut self, insn: u32, avl: u32, register_kind: u32) -> Option<u32> {
        let (kind, immediate_avl) = match insn >> 30 {
            0 | 1 => ((insn >> 20) & 0x7ff, false),
            2 if insn >> 25 == 0x40 => (register_kind, false),
            3 => ((insn >> 20) & 0x3ff, true),
            _ => return None,
        };
        let rs1 = (insn >> 15) & 31;
        let rd = (insn >> 7) & 31;
        let preserve = !immediate_avl && rs1 == 0 && rd == 0;
        let requested = match (immediate_avl, rs1, preserve) {
            (true, _, _) => rs1,
            (false, 0, true) => self.length,
            (false, 0, false) => u32::MAX,
            _ => avl,
        };
        if let Some(maximum) = vlmax(kind).filter(|&max| !preserve || vlmax(self.kind) == Some(max))
        {
            self.kind = kind;
            self.length = requested.min(maximum);
        } else {
            self.kind = VILL;
            self.length = 0;
        }
        self.start = 0;
        Some(self.length)
    }
}

fn vlmax(kind: u32) -> Option<u32> {
    let sew = 8 << ((kind >> 3) & 7);
    if kind & !0xff != 0 || sew > 64 {
        return None;
    }
    let bits = match kind & 7 {
        scale @ 0..=3 => VLEN << scale,
        scale @ 5..=7 if sew <= (64 >> (8 - scale)) => VLEN >> (8 - scale),
        _ => return None,
    };
    Some(bits / sew)
}

const LPDS: u32 = 1;
const PDDS: u32 = 1 << 1;
const CLEAR_FLAGS: u32 = 3 << 2;
const PVD_CONFIG: u32 = 0xf << 4;
const DBP: u32 = 1 << 8;
const FPDS: u32 = 1 << 9;
const VOS: u32 = 1 << 14;
const CONTROL_WRITABLE: u32 = LPDS | PDDS | PVD_CONFIG | DBP | FPDS | VOS;

const BRR: u32 = 1 << 3;
const EWUP: u32 = 1 << 8;
const BRE: u32 = 1 << 9;
const VOSRDY: u32 = 1 << 14;

pub struct F4Power {
    pub base: u32,
    control: u32,
    status: u32,
}

impl F4Power {
    pub const MODEL: &str = "Miscellaneous.STM32_PWR";
    pub const SIZE: u32 = 0x400;

    pub fn new(base: u32) -> Self {
        Self {
            base,
            control: VOS,
            status: 0,
        }
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn read(&self, address: u32, width: usize) -> u32 {
        let offset = address - self.base;
        let value = match offset & !3 {
            0 => self.control,
            4 => self.status,
            _ => 0,
        };
        (value >> ((offset & 3) * 8)) & (u32::MAX >> (32 - width * 8))
    }

    pub fn write(&mut self, address: u32, width: usize, value: u32) {
        let offset = address - self.base;
        let shift = (offset & 3) * 8;
        let lanes = (u32::MAX >> (32 - width * 8)) << shift;
        let value = (value << shift) & lanes;
        match offset & !3 {
            0 => {
                let mask = lanes & CONTROL_WRITABLE;
                self.control = (self.control & !mask) | (value & mask);
                self.status &= !((value & CLEAR_FLAGS) >> 2);
                // Model voltage scaling readiness without regulator settling time.
                if lanes & VOS != 0 {
                    self.status |= VOSRDY;
                }
            }
            4 => {
                let mut writable = EWUP;
                if self.control & DBP != 0 {
                    writable |= BRE;
                }
                let mask = lanes & writable;
                self.status = (self.status & !mask) | (value & mask);
                // Backup regulator enable and ready change together.
                self.status = (self.status & !BRR) | ((self.status & BRE) >> 6);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_commands_and_partial_writes_preserve_unrelated_fields() {
        let mut pwr = F4Power::new(0);
        assert_eq!(pwr.read(0, 4), VOS);
        assert_eq!(pwr.read(4, 4), 0);
        pwr.status = 3; // Pending wake-up and standby flags.
        pwr.write(0, 1, 0x07);
        assert_eq!(pwr.read(0, 4), VOS | LPDS | PDDS);
        assert_eq!(pwr.read(4, 4), 2);
        pwr.write(0, 1, 0x08);
        assert_eq!(pwr.read(4, 4), 0);
        pwr.write(1, 1, 0x81); // Reserved bit 15 plus DBP, with VOS cleared.
        assert_eq!(pwr.read(0, 2), DBP);
        assert_eq!(pwr.read(5, 1), VOSRDY >> 8);
    }

    #[test]
    fn backup_regulator_enable_is_protected_by_dbp() {
        let mut pwr = F4Power::new(0);
        pwr.write(4, 4, EWUP | BRE | BRR);
        assert_eq!(pwr.read(4, 4), EWUP);
        pwr.write(0, 4, VOS | DBP);
        pwr.write(5, 1, (EWUP | BRE) >> 8);
        assert_eq!(pwr.read(4, 4), VOSRDY | EWUP | BRE | BRR);
        pwr.write(5, 1, EWUP >> 8);
        assert_eq!(pwr.read(4, 4), VOSRDY | EWUP);
    }
}

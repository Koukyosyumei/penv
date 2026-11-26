#[derive(Clone, Default)]
pub(super) struct Mpu {
    control: u32,
    selected: usize,
    regions: [(u32, u32); 8],
}

impl Mpu {
    pub(super) fn read(&self, offset: u32) -> Option<u32> {
        let (base, attrs) = self.regions[self.selected];
        Some(match offset {
            0 => 8 << 8,
            4 => self.control,
            8 => self.selected as u32,
            12 => base | self.selected as u32,
            16 => attrs,
            _ => return None,
        })
    }

    pub(super) fn write(&mut self, offset: u32, value: u32) -> bool {
        match offset {
            4 if value & !7 == 0 && value & 2 == 0 => self.control = value,
            8 if value < 8 => self.selected = value as usize,
            12 => {
                if value & 0x10 != 0 {
                    let index = (value & 15) as usize;
                    if index >= self.regions.len() {
                        return false;
                    }
                    self.selected = index;
                }
                self.regions[self.selected].0 = value & !31;
            }
            16 => self.regions[self.selected].1 = value & 0x173f_ff3f,
            _ => return false,
        }
        true
    }

    pub(super) fn permits(&self, address: u32, width: usize, write: bool, execute: bool) -> bool {
        if self.control & 1 == 0 {
            return true;
        }
        (0..width).all(|offset| {
            let Some(address) = address.checked_add(offset as u32) else {
                return false;
            };
            // Privileged System Control Space accesses use the architectural background map.
            if (0xe000_0000..0xe010_0000).contains(&address) {
                return !execute;
            }
            for &(base, attrs) in self.regions.iter().rev() {
                let size_field = (attrs >> 1) & 31;
                if attrs & 1 == 0 || size_field < 4 {
                    continue;
                }
                let size = 1u64 << (size_field + 1);
                let aligned_base = u64::from(base) & !(size - 1);
                let offset = u64::from(address).wrapping_sub(aligned_base);
                if offset >= size || (size >= 256 && attrs & (1 << (8 + offset / (size / 8))) != 0)
                {
                    continue;
                }
                let access = (attrs >> 24) & 7;
                return (!execute || attrs & (1 << 28) == 0)
                    && match access {
                        1..=3 => true,
                        5..=7 => !write,
                        _ => false,
                    };
            }
            self.control & 4 != 0
                && (!execute
                    || address < 0x4000_0000
                    || (0x6000_0000..0xa000_0000).contains(&address))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_region_priority_subregions_and_execute_never() {
        let mut mpu = Mpu::default();
        assert!(mpu.write(4, 5));
        assert!(mpu.write(12, 0x2000_0010));
        assert!(mpu.write(16, (3 << 24) | (7 << 1) | 1));
        assert!(mpu.write(12, 0x2000_0011));
        assert!(mpu.write(16, (1 << 28) | (5 << 24) | (1 << 8) | (7 << 1) | 1));
        assert!(mpu.permits(0x2000_0000, 4, true, true));
        assert!(mpu.permits(0x2000_0020, 4, false, false));
        assert!(!mpu.permits(0x2000_001e, 4, true, false));
        assert!(!mpu.permits(0x2000_0020, 4, false, true));
        assert!(mpu.permits(0xe000_ed90, 4, false, false));
    }
}

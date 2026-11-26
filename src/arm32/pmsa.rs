//! Cortex-R52 PMSAv8 region register banks.

#[derive(Clone, Copy)]
pub(super) enum Register {
    Type,
    Selector,
    Enable,
    Base(Option<usize>),
    Limit(Option<usize>),
    Attribute(usize),
}

impl Register {
    pub(super) fn named(name: &str) -> Option<(usize, Self)> {
        let (bank, name) = name.strip_prefix('h').map_or((0, name), |name| (1, name));
        let register = match name {
            "mpuir" => Self::Type,
            "prselr" => Self::Selector,
            "prenr" => Self::Enable,
            "mair0" => Self::Attribute(0),
            "mair1" => Self::Attribute(1),
            "prbar" => Self::Base(None),
            "prlar" => Self::Limit(None),
            _ => {
                if let Some(index) = name.strip_prefix("prbar") {
                    Self::Base(Some(index.parse().ok()?))
                } else {
                    Self::Limit(Some(name.strip_prefix("prlar")?.parse().ok()?))
                }
            }
        };
        Some((bank, register))
    }

    pub(super) fn decode(key: u32) -> Option<(usize, Self)> {
        let op1 = (key >> 21) & 7;
        let bank = (op1 >> 2) as usize;
        let crn = (key >> 16) & 15;
        let crm = key & 15;
        let op2 = (key >> 5) & 7;
        let register = match (crn, op1 & 3, crm, op2) {
            (0, 0, 0, 4) => Self::Type,
            (6, 0, 2, 1) => Self::Selector,
            (6, 0, 1, 1) => Self::Enable,
            (6, 0, 3, 0) => Self::Base(None),
            (6, 0, 3, 1) => Self::Limit(None),
            (10, 0, 2, 0..=1) => Self::Attribute(op2 as usize),
            (6, 0..=1, 8..=15, 0 | 1 | 4 | 5) => {
                let index = (((op1 & 1) << 4) | ((crm - 8) << 1) | (op2 >> 2)) as usize;
                if op2 & 1 == 0 {
                    Self::Base(Some(index))
                } else {
                    Self::Limit(Some(index))
                }
            }
            _ => return None,
        };
        Some((bank, register))
    }
}

#[derive(Clone, Default)]
struct Region {
    base: u32,
    limit: u32,
}

#[derive(Clone)]
pub(super) struct Pmsa {
    selector: usize,
    implemented: usize,
    attributes: [u32; 2],
    regions: Vec<Region>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Access {
    Fetch,
    Read,
    Write,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Fault {
    Translation = 4,
    Permission = 12,
    Alignment = 33,
}

impl Pmsa {
    pub(super) fn save_snapshot(&self, output: &mut impl std::io::Write) -> std::io::Result<()> {
        use crate::snapshot::write_words;
        write_words(output, &[self.implemented as u64, self.selector as u64])?;
        write_words(output, &self.attributes.map(u64::from))?;
        for region in &self.regions {
            write_words(output, &[region.base, region.limit].map(u64::from))?;
        }
        Ok(())
    }

    pub(super) fn load_snapshot(&mut self, input: &mut impl std::io::Read) -> std::io::Result<()> {
        use crate::snapshot::{invalid, read_words};
        let [implemented, selector] = read_words(input)?;
        if implemented != self.implemented as u64 || selector > 255 {
            return Err(invalid("invalid snapshot ARM MPU configuration"));
        }
        self.selector = selector as usize;
        let word = |value| u32::try_from(value).map_err(std::io::Error::other);
        let [first, second] = read_words(input)?;
        self.attributes = [word(first)?, word(second)?];
        for region in &mut self.regions {
            let [base, limit] = read_words(input)?;
            region.base = word(base)?;
            region.limit = word(limit)?;
        }
        Ok(())
    }

    pub(super) fn check(
        &self,
        address: u32,
        width: u32,
        access: Access,
        user: bool,
        background: bool,
        control: u32,
    ) -> Result<(), Fault> {
        let end = address.checked_add(width - 1).ok_or(Fault::Translation)?;
        self.check_address(address, address, width, access, user, background, control)?;
        if address >> 6 != end >> 6 {
            self.check_address(end, address, width, access, user, background, control)?;
        }
        Ok(())
    }

    fn check_address(
        &self,
        address: u32,
        start: u32,
        width: u32,
        access: Access,
        user: bool,
        background: bool,
        control: u32,
    ) -> Result<(), Fault> {
        let mut matches = self.regions[..self.implemented].iter().filter(|region| {
            region.limit & 1 != 0 && ((region.base & !63)..=(region.limit | 63)).contains(&address)
        });
        let Some(region) = matches.next() else {
            return if background {
                Ok(())
            } else {
                Err(Fault::Permission)
            };
        };
        if matches.next().is_some() {
            return Err(Fault::Translation);
        }
        let permission = (region.base >> 1) & 3;
        let readable = !user || permission & 1 != 0;
        let writable = readable && permission & 2 == 0;
        let execute_never = region.base & 1 != 0
            || writable && control & (1 << 19) != 0
            || !user && permission == 1 && control & (1 << 20) != 0;
        let attr_index = ((region.limit >> 1) & 7) as usize;
        let attr = (self.attributes[attr_index / 4] >> (8 * (attr_index % 4))) as u8;
        let device = attr >> 4 == 0;
        let permitted = match access {
            Access::Fetch => readable && !execute_never && !device,
            Access::Read => readable,
            Access::Write => writable,
        };
        if !permitted {
            return Err(Fault::Permission);
        }
        if device && !start.is_multiple_of(width) {
            return Err(Fault::Alignment);
        }
        Ok(())
    }

    pub(super) fn new(count: usize) -> Self {
        Self {
            selector: 0,
            implemented: count,
            attributes: [0; 2],
            regions: vec![Region::default(); 32],
        }
    }

    pub(super) fn access(&mut self, register: Register, value: Option<u32>) -> Option<Option<u32>> {
        let reply = match register {
            Register::Type => {
                return value
                    .is_none()
                    .then_some(Some((self.implemented as u32) << 8));
            }
            Register::Selector => {
                if let Some(value) = value {
                    self.selector = (value & 255) as usize;
                }
                self.selector as u32
            }
            Register::Enable => {
                if let Some(value) = value {
                    for (index, region) in self.regions.iter_mut().enumerate() {
                        region.limit = (region.limit & !1) | ((value >> index) & 1);
                    }
                }
                self.regions
                    .iter()
                    .enumerate()
                    .fold(0, |mask, (index, region)| {
                        mask | ((region.limit & 1) << index)
                    })
            }
            Register::Base(index) => {
                let Some(region) = self.regions.get_mut(index.unwrap_or(self.selector)) else {
                    return Some(value.is_none().then_some(0));
                };
                if let Some(value) = value {
                    region.base = value & !0x20;
                }
                region.base
            }
            Register::Limit(index) => {
                let Some(region) = self.regions.get_mut(index.unwrap_or(self.selector)) else {
                    return Some(value.is_none().then_some(0));
                };
                if let Some(value) = value {
                    region.limit = value & !0x30;
                }
                region.limit
            }
            Register::Attribute(index) => {
                if let Some(value) = value {
                    self.attributes[index] = value;
                }
                self.attributes[index]
            }
        };
        Some(value.is_none().then_some(reply))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arm_pmsa_checks_user_permissions_alignment_and_overlapping_regions() {
        let mut pmsa = Pmsa::new(2);
        pmsa.access(Register::Attribute(0), Some(0x0000_ff00));
        pmsa.access(Register::Base(Some(0)), Some(0x1002));
        pmsa.access(Register::Limit(Some(0)), Some(0x1003));
        assert_eq!(pmsa.check(0x1000, 4, Access::Fetch, true, false, 0), Ok(()));
        assert_eq!(pmsa.check(0x103f, 1, Access::Write, true, false, 0), Ok(()));
        assert_eq!(
            pmsa.check(0x103f, 2, Access::Read, true, false, 0),
            Err(Fault::Permission)
        );
        pmsa.access(Register::Base(Some(1)), Some(0x1000));
        pmsa.access(Register::Limit(Some(1)), Some(0x1001));
        assert_eq!(
            pmsa.check(0x1000, 4, Access::Read, false, true, 0),
            Err(Fault::Translation)
        );
        pmsa.access(Register::Base(Some(1)), Some(0x1040));
        pmsa.access(Register::Limit(Some(1)), Some(0x1041));
        assert_eq!(
            pmsa.check(0x1042, 4, Access::Read, false, true, 0),
            Err(Fault::Alignment)
        );
        assert_eq!(
            pmsa.check(0x1040, 4, Access::Read, true, false, 0),
            Err(Fault::Permission)
        );
    }
}

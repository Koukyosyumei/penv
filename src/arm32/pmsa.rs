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
    attributes: [u32; 2],
    regions: Vec<Region>,
}

impl Pmsa {
    pub(super) fn new(count: usize) -> Self {
        Self {
            selector: 0,
            attributes: [0; 2],
            regions: vec![Region::default(); count],
        }
    }

    pub(super) fn access(&mut self, register: Register, value: Option<u32>) -> Option<Option<u32>> {
        let reply = match register {
            Register::Type => {
                return value
                    .is_none()
                    .then_some(Some((self.regions.len() as u32) << 8));
            }
            Register::Selector => {
                if let Some(value) = value {
                    self.selector = (value & 31) as usize;
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
                let region = self.regions.get_mut(index.unwrap_or(self.selector))?;
                if let Some(value) = value {
                    region.base = value & !0x20;
                }
                region.base
            }
            Register::Limit(index) => {
                let region = self.regions.get_mut(index.unwrap_or(self.selector))?;
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

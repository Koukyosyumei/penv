pub const READ: u8 = 1;
pub const WRITE: u8 = 2;
pub const EXECUTE: u8 = 4;
const LOCK: u8 = 0x80;
const TOR: u8 = 1;
const MML: u8 = 1;
const MMWP: u8 = 2;
const RLB: u8 = 4;
const SHARED_READ_ONLY: u8 = LOCK | READ | WRITE | EXECUTE;

#[derive(Clone)]
pub struct Pmp {
    config: [u8; 64],
    address: [u32; 64],
    active: u64,
    entries: usize,
    mseccfg: Option<u8>,
    pub external_path: Option<String>,
}

impl Default for Pmp {
    fn default() -> Self {
        Self::new(16)
    }
}

impl Pmp {
    pub fn new(entries: usize) -> Self {
        debug_assert!(matches!(entries, 16 | 64));
        Self {
            config: [0; 64],
            address: [0; 64],
            active: 0,
            entries,
            mseccfg: None,
            external_path: None,
        }
    }

    pub fn enable_smepmp(&mut self) {
        self.mseccfg = Some(0);
    }

    fn write_mseccfg(&mut self, value: u32) -> bool {
        let Some(old) = self.mseccfg else {
            return false;
        };
        let mut allowed = MML | MMWP;
        if old & RLB != 0 || self.config.iter().all(|cfg| cfg & LOCK == 0) {
            allowed |= RLB;
        }
        self.mseccfg = Some((old & (MML | MMWP)) | (value as u8 & allowed));
        true
    }

    fn rule_permissions(&self, config: u8, machine: bool) -> u8 {
        const RWX: u8 = READ | WRITE | EXECUTE;
        let locked = config & LOCK != 0;
        if self.mseccfg.unwrap_or(0) & MML == 0 {
            return if machine && !locked {
                RWX
            } else {
                config & RWX
            };
        }
        if config & SHARED_READ_ONLY == SHARED_READ_ONLY {
            return READ;
        }
        if config & (READ | WRITE) == WRITE {
            let execute_bit = config & EXECUTE != 0;
            return if locked {
                EXECUTE | if machine && execute_bit { READ } else { 0 }
            } else {
                READ | if machine || execute_bit { WRITE } else { 0 }
            };
        }
        if machine == locked { config & RWX } else { 0 }
    }

    pub fn external_csr(&mut self, config: bool, index: u32, value: Option<u32>) -> Option<u32> {
        let limit = if config {
            self.entries / 4
        } else {
            self.entries
        };
        if index as usize >= limit {
            self.log_invalid(config, index, value.is_some());
            return value.is_none().then_some(0);
        }
        let csr = if config { 0x3a0 } else { 0x3b0 } + index;
        if let Some(value) = value {
            self.write(csr, value);
            None
        } else {
            self.read(csr)
        }
    }

    fn log_invalid(&self, config: bool, index: u32, write: bool) {
        let action = if write { "write to" } else { "read from" };
        let register = if config { "config" } else { "address" };
        let message = format!("Attempted to {action} invalid PMP {register} register {index}");
        crate::monitor_log::record(3, &message);
        eprintln!("{message}");
    }

    pub fn is_external_csr(&self, csr: u32) -> bool {
        self.external_path.is_some() && matches!(csr, 0x3a0..=0x3af | 0x3b0..=0x3ef)
    }

    pub fn read(&self, csr: u32) -> Option<u32> {
        Some(match csr {
            0x747 => u32::from(self.mseccfg?),
            0x757 if self.mseccfg.is_some() => 0,
            0x3a0..=0x3af if ((csr - 0x3a0) * 4) < self.entries as u32 => {
                let start = (csr as usize - 0x3a0) * 4;
                u32::from_le_bytes(self.config[start..start + 4].try_into().unwrap())
            }
            0x3b0..=0x3ef if csr - 0x3b0 < self.entries as u32 => {
                self.address[(csr - 0x3b0) as usize]
            }
            _ if self.is_external_csr(csr) => {
                let config = csr < 0x3b0;
                self.log_invalid(config, csr - if config { 0x3a0 } else { 0x3b0 }, false);
                0
            }
            _ => return None,
        })
    }

    fn locked_by_next(&self, index: usize) -> bool {
        self.config[..self.entries]
            .get(index + 1)
            .is_some_and(|&config| config & LOCK != 0 && mode(config) == TOR)
    }

    pub fn write(&mut self, csr: u32, value: u32) -> bool {
        let security = self.mseccfg.unwrap_or(0);
        let bypass = security & RLB != 0;
        let lockdown = security & MML != 0;
        match csr {
            0x747 => return self.write_mseccfg(value),
            0x757 if self.mseccfg.is_some() => {}
            0x3a0..=0x3af if ((csr - 0x3a0) * 4) < self.entries as u32 => {
                let start = (csr as usize - 0x3a0) * 4;
                for (index, config) in (start..start + 4).zip(value.to_le_bytes()) {
                    if !bypass
                        && (self.config[index] & LOCK != 0
                            || (self.external_path.is_some() && self.locked_by_next(index)))
                    {
                        continue;
                    }
                    let mut config = config & 0x9f;
                    let shared_encoding = config & (READ | WRITE) == WRITE;
                    if self.external_path.is_none() && !lockdown && shared_encoding {
                        config &= !WRITE;
                    }
                    if lockdown
                        && !bypass
                        && config & LOCK != 0
                        && (config & EXECUTE != 0 || shared_encoding)
                        && config & SHARED_READ_ONLY != SHARED_READ_ONLY
                    {
                        continue;
                    }
                    self.config[index] = config;
                    self.active &= !(1 << index);
                    if mode(config) != 0 {
                        self.active |= 1 << index;
                    }
                }
            }
            0x3b0..=0x3ef if csr - 0x3b0 < self.entries as u32 => {
                let index = (csr - 0x3b0) as usize;
                if bypass || (self.config[index] & LOCK == 0 && !self.locked_by_next(index)) {
                    self.address[index] = value;
                }
            }
            _ if self.is_external_csr(csr) => {
                let config = csr < 0x3b0;
                self.log_invalid(config, csr - if config { 0x3a0 } else { 0x3b0 }, true);
            }
            _ => return false,
        }
        true
    }

    pub fn permits(&self, address: u32, width: usize, permission: u8, machine: bool) -> bool {
        if self.external_path.is_some() && self.active == 0 {
            return true;
        }
        let start = u64::from(address);
        let end = start + width as u64;
        let mut active = self.active;
        while active != 0 {
            let index = active.trailing_zeros() as usize;
            active &= active - 1;
            let config = self.config[index];
            let (lower, upper) = self.range(index, mode(config));
            if lower >= upper || end <= lower || start >= upper {
                continue;
            }
            return start >= lower
                && end <= upper
                && self.rule_permissions(config, machine) & permission == permission;
        }
        let security = self.mseccfg.unwrap_or(0);
        machine && security & MMWP == 0 && (security & MML == 0 || permission & EXECUTE == 0)
    }

    fn range(&self, index: usize, mode: u8) -> (u64, u64) {
        let address = u64::from(self.address[index]);
        match mode {
            TOR => {
                let lower = if index == 0 {
                    0
                } else {
                    u64::from(self.address[index - 1]) << 2
                };
                (lower, address << 2)
            }
            2 => (address << 2, (address << 2) + 4),
            _ => {
                let mask = (1u64 << address.trailing_ones()) - 1;
                let lower = (address & !mask) << 2;
                (lower, lower + ((mask + 1) << 3))
            }
        }
    }
}

fn mode(config: u8) -> u8 {
    (config >> 3) & 3
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_pmp_enforces_active_rules_and_locked_tor_predecessor() {
        let mut pmp = Pmp::new(16);
        pmp.external_path = Some("cpu.pmp".into());
        assert!(pmp.permits(0x1000, 4, READ, false));
        assert!(pmp.write(0x3b0, 0x400));
        assert!(pmp.write(0x3b1, 0x800));
        assert!(pmp.write(0x3a0, 0x8900)); // Locked TOR [0x1000, 0x2000), read only.
        assert!(pmp.permits(0x1000, 4, READ, false));
        assert!(!pmp.permits(0x1000, 4, WRITE, true));
        assert!(!pmp.permits(0x2000, 4, READ, false));
        assert!(pmp.write(0x3a0, 0x1f));
        assert!(pmp.write(0x3b0, 0));
        assert_eq!(pmp.read(0x3a0), Some(0x8900));
        assert_eq!(pmp.read(0x3b0), Some(0x400));
    }

    #[test]
    fn smepmp_lockdown_permissions_match_the_architectural_table() {
        const R: u8 = READ;
        const W: u8 = WRITE;
        const X: u8 = EXECUTE;
        let machine = [
            0,
            0,
            R | W,
            R | W,
            0,
            0,
            0,
            0,
            0,
            X,
            X,
            R | X,
            R,
            R | X,
            R | W,
            R,
        ];
        let lower = [
            0,
            X,
            R,
            R | W,
            R,
            R | X,
            R | W,
            R | W | X,
            0,
            0,
            X,
            X,
            0,
            0,
            0,
            R,
        ];
        let mut pmp = Pmp::new(16);
        pmp.enable_smepmp();
        assert!(pmp.write_mseccfg(u32::from(MML | RLB)));
        assert!(pmp.write(0x3b0, 0x800));
        for (index, (&m, &su)) in machine.iter().zip(&lower).enumerate() {
            let rwx = ((index & 4) >> 2) | (index & 2) | ((index & 1) << 2);
            let cfg = (index & 8) << 4 | rwx | 0x08;
            assert!(pmp.write(0x3a0, cfg as u32));
            for (mode, expected) in [(true, m), (false, su)] {
                for bit in [R, W, X, R | W] {
                    assert_eq!(
                        pmp.permits(0x1000, 4, bit, mode),
                        expected & bit == bit,
                        "{cfg:#x}"
                    );
                }
            }
        }
    }

    #[test]
    fn smepmp_bypass_is_locked_out_after_a_locked_tor_rule() {
        let mut pmp = Pmp::new(16);
        pmp.enable_smepmp();
        assert!(pmp.write_mseccfg(u32::from(RLB)));
        assert!(pmp.write(0x3a0, 0x8900));
        assert!(pmp.write(0x3b0, 0x400));
        assert!(pmp.write(0x3b1, 0x800));
        assert!(pmp.write_mseccfg(u32::from(MML | MMWP)));
        assert!(pmp.write_mseccfg(u32::MAX));
        assert_eq!(pmp.mseccfg, Some(MML | MMWP));
        assert!(pmp.write_mseccfg(0));
        assert_eq!(pmp.mseccfg, Some(MML | MMWP));
        assert!(pmp.write(0x3b0, 1));
        assert!(pmp.write(0x3b1, 2));
        assert_eq!(
            (pmp.read(0x3b0), pmp.read(0x3b1)),
            (Some(0x400), Some(0x800))
        );
        assert!(pmp.write(0x3a0, 0x008c_0000));
        assert_eq!(pmp.read(0x3a0), Some(0x8900)); // executable M-only rule rejected
        assert!(pmp.permits(0x1000, 4, READ, true));
        assert!(!pmp.permits(0x1000, 4, READ, false));
        assert!(!pmp.permits(0x3000, 4, READ, true));
    }

    #[test]
    fn smepmp_unmatched_policy_and_disabled_lock() {
        let mut pmp = Pmp::new(16);
        pmp.enable_smepmp();
        assert!(pmp.write(0x3a0, u32::from(LOCK))); // locked, A=OFF
        assert!(pmp.write_mseccfg(u32::from(MML | RLB)));
        assert_eq!(pmp.mseccfg, Some(MML));
        assert!(pmp.permits(0x1000, 4, READ | WRITE, true));
        assert!(!pmp.permits(0x1000, 4, EXECUTE, true));
        assert!(pmp.write_mseccfg(u32::from(MMWP)));
        assert!(!pmp.permits(0x1000, 4, READ, true));
    }

    #[test]
    fn honors_first_overlap_and_locked_tor_address() {
        let mut pmp = Pmp::default();
        assert!(!pmp.permits(0x1000, 4, READ, false));
        assert!(pmp.write(0x3b0, 0x400));
        assert!(pmp.write(0x3b1, 0x402));
        assert!(pmp.write(0x3a0, 0x8900)); // entry 1: locked, TOR, readable
        assert!(pmp.write(0x3b0, 0));
        assert_eq!(pmp.read(0x3b0), Some(0x400));
        assert!(pmp.permits(0x1000, 4, READ, false));
        assert!(!pmp.permits(0x1000, 4, WRITE, true));
        assert!(!pmp.permits(0x1004, 8, READ, true));
        assert!(pmp.write(0x3b2, 0x403)); // NAPOT [0x1000, 0x1020)
        assert!(pmp.write(0x3a0, 0x1b_0000)); // entry 2: readable and writable
        assert!(pmp.permits(0x1008, 4, WRITE, false));
        assert!(!pmp.permits(0x1004, 8, WRITE, false));
    }
}

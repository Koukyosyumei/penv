use std::cell::Cell;

use crate::{
    bus::Bus,
    pmp::{EXECUTE, READ, WRITE},
    rv64::{Fault, ram_read},
    rv64_priv::{MXR, Mode, Privilege, SUM},
};

const ENTRIES: usize = 64;

#[derive(Clone, Copy, Default)]
struct Entry {
    satp: u64,
    virtual_page: u64,
    physical_page: u64,
    flags: u8,
}

#[derive(Clone)]
pub struct TranslationCache {
    entries: [Cell<Entry>; ENTRIES],
}

impl Default for TranslationCache {
    fn default() -> Self {
        Self {
            entries: std::array::from_fn(|_| Cell::new(Entry::default())),
        }
    }
}

impl TranslationCache {
    pub fn clear(&self) {
        for entry in &self.entries {
            entry.set(Entry::default());
        }
    }

    pub fn translate(
        &self,
        bus: &mut Bus,
        csr: &Privilege,
        address: u64,
        width: usize,
        permission: u8,
    ) -> Result<u64, Fault> {
        let mode = csr.effective_mode(permission);
        let satp = csr.satp;
        let (access_cause, page_cause) = match permission {
            EXECUTE => (1, 12),
            READ => (5, 13),
            _ => (7, 15),
        };
        let physical = if mode == Mode::Machine || satp >> 60 == 0 {
            address
        } else {
            let virtual_page = address >> 12;
            let flags =
                permission | ((mode as u8) << 3) | (((csr.status & (SUM | MXR)) >> 14) as u8);
            let index = (virtual_page ^ (virtual_page >> 6) ^ (u64::from(permission) * 13))
                as usize
                & (ENTRIES - 1);
            let entry = &self.entries[index];
            let cached = entry.get();
            let physical_page = if cached.satp == satp
                && cached.virtual_page == virtual_page
                && cached.flags == flags
            {
                cached.physical_page
            } else {
                let physical = walk(bus, csr, address, mode, permission).map_err(|failure| {
                    Fault::new(
                        if failure == WalkFault::Access {
                            access_cause
                        } else {
                            page_cause
                        },
                        address,
                    )
                })?;
                entry.set(Entry {
                    satp,
                    virtual_page,
                    physical_page: physical,
                    flags,
                });
                physical
            };
            physical_page | (address & 0xfff)
        };
        permitted(csr, physical, width, permission, mode)
            .then_some(physical)
            .ok_or_else(|| Fault::new(access_cause, address))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WalkFault {
    Access,
    Page,
}

fn walk(
    bus: &mut Bus,
    csr: &Privilege,
    address: u64,
    mode: Mode,
    permission: u8,
) -> Result<u64, WalkFault> {
    let satp_mode = csr.satp >> 60;
    let levels = (satp_mode - 5) as u32;
    let va_bits = 12 + levels * 9;
    let mask = (1 << va_bits) - 1;
    let sign = (address >> (va_bits - 1)) & 1;
    if address & !mask != (0u64.wrapping_sub(sign) & !mask) {
        return Err(WalkFault::Page);
    }
    let ppn_mask = (1 << 44) - 1;
    let mut table = (csr.satp & ppn_mask) << 12;
    let vpn = (address & mask) >> 12;
    for level in (0..levels).rev() {
        let index = (vpn >> (level * 9)) & 511;
        let pte_address = table + index * 8;
        if !permitted(csr, pte_address, 8, READ, Mode::Supervisor) {
            return Err(WalkFault::Access);
        }
        let pte = ram_read(bus, None, pte_address, 8).ok_or(WalkFault::Access)?;
        let rights = ((pte >> 1) & 7) as u8;
        let ppn = (pte >> 10) & ppn_mask;
        if pte & 1 == 0 || (rights & (READ | WRITE)) == WRITE || pte >> 54 != 0 {
            return Err(WalkFault::Page);
        }
        if rights == 0 {
            if level == 0 || pte & 0xd0 != 0 {
                return Err(WalkFault::Page);
            }
            table = ppn << 12;
            continue;
        }
        let user = pte & 0x10 != 0;
        let allowed_mode = if mode == Mode::User {
            user
        } else {
            !user || (permission != EXECUTE && csr.status & SUM != 0)
        };
        let rights = if csr.status & MXR != 0 && rights & EXECUTE != 0 {
            rights | READ
        } else {
            rights
        };
        let lower = (1 << (level * 9)) - 1;
        if !allowed_mode || rights & permission != permission || ppn & lower != 0 {
            return Err(WalkFault::Page);
        }
        let bits = if permission & WRITE != 0 { 0xc0 } else { 0x40 };
        if pte & bits != bits {
            if !permitted(csr, pte_address, 8, WRITE, Mode::Supervisor) {
                return Err(WalkFault::Access);
            }
            bus.memory64(pte_address, 8)
                .ok_or(WalkFault::Access)?
                .copy_from_slice(&(pte | bits).to_le_bytes());
        }
        return Ok((ppn | (vpn & lower)) << 12);
    }
    Err(WalkFault::Page)
}

fn permitted(csr: &Privilege, address: u64, width: usize, permission: u8, mode: Mode) -> bool {
    csr.pmp
        .as_ref()
        .is_none_or(|pmp| pmp.permits(address, width, permission, mode))
}

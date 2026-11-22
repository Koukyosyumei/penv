use crate::{
    bus::Bus,
    pmp::{EXECUTE, READ, WRITE},
    rv64::{Fault, ram_read},
    rv64_priv::{MXR, Mode, Privilege, SUM},
};

pub fn translate(
    bus: &mut Bus,
    csr: &Privilege,
    address: u64,
    width: usize,
    permission: u8,
) -> Result<u64, Fault> {
    let (access_cause, page_cause) = match permission {
        EXECUTE => (1, 12),
        READ => (5, 13),
        _ => (7, 15),
    };
    let access_fault = || Fault::new(access_cause, address);
    let page_fault = || Fault::new(page_cause, address);
    let mode = csr.effective_mode(permission);
    let satp_mode = csr.satp >> 60;
    let mut physical = address;
    if mode != Mode::Machine && satp_mode != 0 {
        let levels = (satp_mode - 5) as u32;
        let va_bits = 12 + levels * 9;
        let mask = (1 << va_bits) - 1;
        let sign = (address >> (va_bits - 1)) & 1;
        if address & !mask != (0u64.wrapping_sub(sign) & !mask) {
            return Err(page_fault());
        }
        let ppn_mask = (1 << 44) - 1;
        let mut table = (csr.satp & ppn_mask) << 12;
        let vpn = (address & mask) >> 12;
        for level in (0..levels).rev() {
            let index = (vpn >> (level * 9)) & 511;
            let pte_address = table + index * 8;
            if !permitted(csr, pte_address, 8, READ, Mode::Supervisor) {
                return Err(access_fault());
            }
            let pte = ram_read(bus, pte_address, 8).ok_or_else(access_fault)?;
            let rights = ((pte >> 1) & 7) as u8;
            let ppn = (pte >> 10) & ppn_mask;
            if pte & 1 == 0 || (rights & (READ | WRITE)) == WRITE || pte >> 54 != 0 {
                return Err(page_fault());
            }
            if rights == 0 {
                if level == 0 || pte & 0xd0 != 0 {
                    return Err(page_fault());
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
                return Err(page_fault());
            }
            let bits = if permission & WRITE != 0 { 0xc0 } else { 0x40 };
            if pte & bits != bits {
                if !permitted(csr, pte_address, 8, WRITE, Mode::Supervisor) {
                    return Err(access_fault());
                }
                bus.memory64(pte_address, 8)
                    .ok_or_else(access_fault)?
                    .copy_from_slice(&(pte | bits).to_le_bytes());
            }
            physical = ((ppn | (vpn & lower)) << 12) | (address & 0xfff);
            break;
        }
    }
    permitted(csr, physical, width, permission, mode)
        .then_some(physical)
        .ok_or_else(access_fault)
}

fn permitted(csr: &Privilege, address: u64, width: usize, permission: u8, mode: Mode) -> bool {
    csr.pmp
        .as_ref()
        .is_none_or(|pmp| pmp.permits(address, width, permission, mode))
}

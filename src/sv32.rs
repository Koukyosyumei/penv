use crate::{
    bus::{Bus, BusError},
    csr::Csr,
    pmp::{EXECUTE, READ, WRITE},
};

const VALID: u32 = 1;
const USER: u32 = 1 << 4;
const ACCESSED: u32 = 1 << 6;
const DIRTY: u32 = 1 << 7;

const ENTRIES: usize = 64;

#[derive(Clone, Copy, Default)]
struct Entry {
    tag: u64,
    page: u32,
}

#[derive(Clone)]
pub struct TranslationCache {
    entries: [Entry; ENTRIES],
}

impl Default for TranslationCache {
    fn default() -> Self {
        Self {
            entries: [Entry::default(); ENTRIES],
        }
    }
}

impl TranslationCache {
    pub fn clear(&mut self) {
        self.entries.fill(Entry::default());
    }

    pub fn translate(
        &mut self,
        bus: &mut Bus,
        csr: &Csr,
        va: u32,
        access: u8,
    ) -> Result<u32, BusError> {
        let Some((satp, user, sum, mxr)) = csr.translation(access == EXECUTE) else {
            return Ok(va);
        };
        let page = va >> 12;
        let flags = u64::from(access)
            | (u64::from(user) << 3)
            | (u64::from(sum) << 4)
            | (u64::from(mxr) << 5);
        // SATP.MODE is set for cached translations, so a zero tag is invalid.
        let tag = (flags << 52) | (u64::from(satp) << 20) | u64::from(page);
        let index = (page ^ (page >> 6) ^ (u32::from(access) * 13)) as usize & (ENTRIES - 1);
        let entry = &mut self.entries[index];
        if entry.tag == tag {
            return Ok(entry.page | (va & 0xfff));
        }
        let physical = walk(bus, csr, va, access, (satp & 0x3f_ffff, user, sum, mxr))?;
        *entry = Entry {
            tag,
            page: physical & !0xfff,
        };
        Ok(physical)
    }
}

fn walk(
    bus: &mut Bus,
    csr: &Csr,
    va: u32,
    access: u8,
    context: (u32, bool, bool, bool),
) -> Result<u32, BusError> {
    let (root, user, sum, mxr) = context;
    let mut table = u64::from(root) << 12;
    for shift in [22, 12] {
        let offset = (va >> shift) & 0x3ff;
        let address =
            u32::try_from(table + u64::from(offset) * 4).map_err(|_| BusError::Denied(va))?;
        if !csr.permits_page_table(address, READ) {
            return Err(BusError::Denied(va));
        }
        let pte = bus
            .read_ram::<4>(address)
            .map_err(|_| BusError::Denied(va))?;
        let mut permissions = ((pte >> 1) & 7) as u8;
        if pte & VALID == 0 || permissions & (READ | WRITE) == WRITE {
            return Err(BusError::Page(va));
        }
        if permissions == 0 {
            if shift == 12 || pte & (USER | ACCESSED | DIRTY) != 0 {
                return Err(BusError::Page(va));
            }
            table = u64::from(pte >> 10) << 12;
            continue;
        }
        if mxr && permissions & EXECUTE != 0 {
            permissions |= READ;
        }
        let user_page = pte & USER != 0;
        if permissions & access != access
            || (user && !user_page)
            || (!user && user_page && (access == EXECUTE || !sum))
        {
            return Err(BusError::Page(va));
        }
        let mask = (1u32 << shift) - 1;
        let physical_base = u64::from(pte >> 10) << 12;
        if physical_base & u64::from(mask) != 0 {
            return Err(BusError::Page(va));
        }
        let physical = u32::try_from(physical_base | u64::from(va & mask))
            .map_err(|_| BusError::Denied(va))?;
        let update = ACCESSED | if access & WRITE != 0 { DIRTY } else { 0 };
        if pte & update != update {
            if !csr.permits_page_table(address, WRITE) {
                return Err(BusError::Denied(va));
            }
            bus.atomic_update(address, 4, |old| old | update)
                .map_err(|_| BusError::Denied(va))?;
        }
        return Ok(physical);
    }
    Err(BusError::Page(va))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn separates_supervisor_sum_and_mxr_permissions_in_cached_translations() {
        const ROOT: u32 = 0x8000_0000;
        const LEAF: u32 = ROOT + 0x1000;
        const PAGE: u32 = ROOT + 0x2000;
        const VA: u32 = 0x4000_0000;
        const SUM: u32 = 1 << 18;
        const MXR: u32 = 1 << 19;
        let mut bus = Bus::new(0x3000).unwrap();
        for (address, pte) in [
            (ROOT + (VA >> 22) * 4, ((LEAF >> 12) << 10) | VALID),
            (
                LEAF,
                ((PAGE >> 12) << 10) | VALID | USER | (u32::from(EXECUTE) << 1),
            ),
        ] {
            bus.memory(address, 4)
                .unwrap()
                .copy_from_slice(&pte.to_le_bytes());
        }
        let mut csr = Csr::default();
        csr.enable_user_mode();
        csr.enable_supervisor_mode();
        csr.access(0x1800_9073, 0x8000_0000 | (ROOT >> 12)).unwrap();
        csr.access(0x3000_9073, 1 << 11).unwrap();
        csr.return_from_trap().unwrap();
        let mut cache = TranslationCache::default();
        for status in [MXR | SUM, MXR, SUM, 0, MXR | SUM] {
            csr.access(0x1000_9073, status).unwrap();
            let result = cache.translate(&mut bus, &csr, VA, READ);
            if status == MXR | SUM {
                assert_eq!(result.unwrap(), PAGE);
            } else {
                assert!(matches!(result, Err(BusError::Page(VA))));
            }
            assert!(matches!(
                cache.translate(&mut bus, &csr, VA, EXECUTE),
                Err(BusError::Page(VA))
            ));
        }
    }
}

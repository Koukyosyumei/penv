use crate::dma_engine::Width;

#[derive(Clone, Copy)]
pub enum Endian {
    Little,
    Big,
}

fn mask(width: Width) -> u64 {
    u64::MAX >> (64 - width as u32 * 8)
}

fn swap(value: u64, width: Width) -> u64 {
    value.swap_bytes() >> (64 - width as u32 * 8)
}

pub fn little_lane(value: u64, offset: u32, width: usize) -> u64 {
    (value >> (offset * 8)) & (u64::MAX >> (64 - width * 8))
}

/// Translate one access to a peripheral's supported native width.
pub fn read_using(
    address: u64,
    requested: Width,
    native: Width,
    endian: Endian,
    mut read: impl FnMut(u64) -> u64,
) -> u64 {
    let unit = native as u64;
    let length = requested as u64;
    if length < unit {
        let offset = address % unit;
        let value = read(address - offset);
        return match endian {
            Endian::Little => (value >> (8 * offset)) & mask(requested),
            Endian::Big => {
                let lane = (unit - length).wrapping_sub(offset);
                let shift = lane.wrapping_mul(8) % (unit * 8);
                swap((value >> shift) & mask(requested), requested)
            }
        };
    }
    if length == unit {
        return read(address) & mask(native);
    }

    let units = length / unit;
    let mut result = 0;
    for index in 0..units {
        let (offset, swap_bytes) = match endian {
            Endian::Little => (index * unit, false),
            Endian::Big => ((units - index - 1) * unit, true),
        };
        let mut part = read(address.wrapping_add(offset)) & mask(native);
        if swap_bytes {
            part = swap(part, native);
        }
        result |= part << (index * unit * 8);
    }
    result
}

pub trait NativeWrite {
    fn read_for_write(&mut self, address: u64) -> u64;
    fn write_native(&mut self, address: u64, value: u64);
}

pub fn write_using(
    address: u64,
    value: u64,
    requested: Width,
    native: Width,
    endian: Endian,
    peripheral: &mut impl NativeWrite,
) {
    let unit = native as u64;
    let length = requested as u64;
    if length < unit {
        let offset = address % unit;
        let address = address - offset;
        let (lane, value) = match endian {
            Endian::Little => (offset, value),
            Endian::Big => ((unit - length).wrapping_sub(offset), swap(value, requested)),
        };
        let shift = lane.wrapping_mul(8) % (unit * 8);
        let bits = mask(requested) << shift;
        let old_value = peripheral.read_for_write(address);
        peripheral.write_native(
            address,
            ((old_value & !bits) | ((value << shift) & bits)) & mask(native),
        );
        return;
    }
    if length == unit {
        peripheral.write_native(address, value & mask(native));
        return;
    }

    let bytes = match endian {
        Endian::Little => value & mask(requested),
        Endian::Big => swap(value, requested),
    };
    for offset in (0..length).step_by(unit as usize) {
        peripheral.write_native(
            address.wrapping_add(offset),
            (bytes >> (offset * 8)) & mask(native),
        );
    }
}

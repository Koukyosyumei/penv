use super::{Bus, Thumb, Write};

impl Thumb {
    fn shift_immediate(&self, value: u32, kind: u32, shift: u32) -> (u32, bool) {
        match (kind, shift) {
            (0, 0) => (value, self.carry),
            (0, n) => (value << n, value & (1 << (32 - n)) != 0),
            (1, 0) => (0, value >> 31 != 0),
            (1, n) => (value >> n, value & (1 << (n - 1)) != 0),
            (2, 0) => (((value as i32) >> 31) as u32, value >> 31 != 0),
            (2, n) => (((value as i32) >> n) as u32, value & (1 << (n - 1)) != 0),
            (3, 0) => ((value >> 1) | (u32::from(self.carry) << 31), value & 1 != 0),
            (_, n) => {
                let result = value.rotate_right(n);
                (result, result >> 31 != 0)
            }
        }
    }

    pub(super) fn move_wide(&self, high: u32, low: u32, r: &mut [u32; 32]) -> bool {
        let rd = ((low >> 8) & 15) as usize;
        if !matches!(high & 0xfbf0, 0xf240 | 0xf2c0) || low & 0x8000 != 0 || matches!(rd, 13 | 15) {
            return false;
        }
        let value =
            ((high & 15) << 12) | ((high & 0x400) << 1) | ((low & 0x7000) >> 4) | (low & 0xff);
        r[rd] = if high & 0x80 == 0 {
            value
        } else {
            (r[rd] & 0xffff) | (value << 16)
        };
        true
    }

    pub(super) fn insert_bits(&self, high: u32, low: u32, r: &mut [u32; 32]) -> bool {
        let source = (high & 15) as usize;
        let target = ((low >> 8) & 15) as usize;
        let start = ((low >> 10) & 0x1c) | ((low >> 6) & 3);
        let end = low & 31;
        if high & 0xfff0 != 0xf360
            || low & 0x8020 != 0
            || source == 13
            || matches!(target, 13 | 15)
            || end < start
        {
            return false;
        }
        let mask = (u32::MAX << start) & (u32::MAX >> (31 - end));
        let value = if source == 15 { 0 } else { r[source] << start };
        r[target] = (r[target] & !mask) | (value & mask);
        true
    }

    pub(super) fn extract_bits(&self, high: u32, low: u32, r: &mut [u32; 32]) -> bool {
        let source = (high & 15) as usize;
        let target = ((low >> 8) & 15) as usize;
        let start = ((low >> 10) & 0x1c) | ((low >> 6) & 3);
        let width = (low & 31) + 1;
        if !matches!(high & 0xfff0, 0xf340 | 0xf3c0)
            || low & 0x8020 != 0
            || matches!(source, 13 | 15)
            || matches!(target, 13 | 15)
            || start + width > 32
        {
            return false;
        }
        let left = r[source] << (32 - start - width);
        r[target] = if high & 0x80 == 0 {
            ((left as i32) >> (32 - width)) as u32
        } else {
            left >> (32 - width)
        };
        true
    }

    pub(super) fn modified_immediate(&mut self, high: u32, low: u32, r: &mut [u32; 32]) -> bool {
        if high & 0xfa00 != 0xf000 || low & 0x8000 != 0 {
            return false;
        }
        let op = (high >> 5) & 15;
        let rn = (high & 15) as usize;
        let rd = ((low >> 8) & 15) as usize;
        let update_flags = high & 0x10 != 0;
        let compare = rd == 15 && update_flags && matches!(op, 0 | 4 | 8 | 13);
        if (rd == 15 && !compare) || (rn == 15 && !matches!(op, 2 | 3)) {
            return false;
        }
        let encoded = ((high & 0x400) << 1) | ((low & 0x7000) >> 4) | (low & 0xff);
        let byte = encoded & 0xff;
        let (immediate, carry) = if encoded < 0x400 {
            let pattern = match encoded >> 8 {
                0 => byte,
                1 => byte * 0x0001_0001,
                2 => byte * 0x0100_0100,
                _ => byte * 0x0101_0101,
            };
            (pattern, self.carry)
        } else {
            let rotated = (0x80 | (encoded & 0x7f)).rotate_right(encoded >> 7);
            (rotated, rotated >> 31 != 0)
        };
        let lhs = if rn == 15 { 0 } else { r[rn] };
        let result = match op {
            0 => lhs & immediate,
            1 => lhs & !immediate,
            2 => lhs | immediate,
            3 => lhs | !immediate,
            4 => lhs ^ immediate,
            10 | 11 if !compare => {
                let flags = self.apsr();
                r[rd] = self
                    .data_processing(if op == 10 { 5 } else { 6 }, lhs, immediate)
                    .unwrap();
                if !update_flags {
                    self.set_apsr(flags);
                }
                return true;
            }
            8 | 13 | 14 => {
                let (lhs, immediate) = if op == 14 {
                    (immediate, lhs)
                } else {
                    (lhs, immediate)
                };
                let result = if update_flags {
                    self.arithmetic(lhs, immediate, op != 8)
                } else if op != 8 {
                    lhs.wrapping_sub(immediate)
                } else {
                    lhs.wrapping_add(immediate)
                };
                if !compare {
                    r[rd] = result;
                }
                return true;
            }
            _ => return false,
        };
        if !compare {
            r[rd] = result;
        }
        if update_flags {
            self.nz(result);
            self.carry = carry;
        }
        true
    }

    pub(super) fn execute_thumb2(
        &mut self,
        high: u32,
        low: u32,
        next: &mut u32,
        r: &mut [u32; 32],
        bus: &mut Bus,
        output: &mut impl Write,
    ) -> Result<(), String> {
        let unsupported = || format!("unsupported Thumb instruction {high:#06x} {low:#06x}");
        let rn = (high & 15) as usize;
        let rt = (low >> 12) as usize;
        let rm = (low & 15) as usize;
        let rd = ((low >> 8) & 15) as usize;
        match high {
            0xe8d0..=0xe8df if low & 0xffe0 == 0xf000 && !matches!(rm, 13 | 15) && rn != 13 => {
                let base = if rn == 15 { *next } else { r[rn] };
                let offset = if low & 0x10 == 0 {
                    bus.thumb_read::<1>(base.wrapping_add(r[rm]))?
                } else {
                    bus.thumb_read::<2>(base.wrapping_add(r[rm].wrapping_mul(2)))?
                };
                *next = next.wrapping_add(offset * 2);
            }
            0xe850..=0xe85e if low & 0xf00 == 0xf00 && !matches!(rt, 13 | 15) => {
                let address = r[rn].wrapping_add((low & 0xff) * 4);
                bus.check_thumb_access(address, 4, false, false)?;
                r[rt] = bus.load_reserved(0, address).map_err(|e| e.to_string())?;
            }
            0xe840..=0xe84e
                if !matches!(rt, 13 | 15) && !matches!(rd, 13 | 15) && rd != rt && rd != rn =>
            {
                let address = r[rn].wrapping_add((low & 0xff) * 4);
                bus.check_thumb_access(address, 4, true, false)?;
                r[rd] = bus
                    .store_conditional(0, address, r[rt])
                    .map_err(|e| e.to_string())?;
            }
            0xe880..=0xe8be if low.count_ones() >= 2 && low & 0xa000 == 0 && rn < 15 => {
                let writeback = high & 0x20 != 0;
                let load = high & 0x10 != 0;
                if writeback && low & (1 << rn) != 0 {
                    return Err(unsupported());
                }
                let mut address = r[rn];
                if address & 3 != 0 {
                    return Err(format!(
                        "unaligned multiple-register transfer at {address:#010x}"
                    ));
                }
                let mut values = *r;
                for register in (0..15).filter(|register| low & (1 << register) != 0) {
                    if load {
                        values[register] = bus.thumb_read::<4>(address)?;
                    } else {
                        bus.thumb_write(address, 4, r[register], output)?;
                    }
                    address = address.wrapping_add(4);
                }
                if writeback {
                    values[rn] = address;
                }
                *r = values;
            }
            0xe940..=0xe97e | 0xe9c0..=0xe9fe
                if rn < 15
                    && !matches!(rt, 13 | 15)
                    && !matches!(rd, 13 | 15)
                    && (high & 0x10 == 0 || rt != rd)
                    && (high & 0x20 == 0 || (rn != rt && rn != rd)) =>
            {
                let offset = (low & 0xff) * 4;
                let address = if high & 0x80 == 0 {
                    r[rn].wrapping_sub(offset)
                } else {
                    r[rn].wrapping_add(offset)
                };
                if address & 3 != 0 {
                    return Err(format!("unaligned double-word transfer at {address:#010x}"));
                }
                let second = address.wrapping_add(4);
                if high & 0x10 != 0 {
                    let first = bus.thumb_read::<4>(address)?;
                    let second = bus.thumb_read::<4>(second)?;
                    r[rt] = first;
                    r[rd] = second;
                } else {
                    bus.thumb_write(address, 4, r[rt], output)?;
                    bus.thumb_write(second, 4, r[rd], output)?;
                }
                if high & 0x20 != 0 {
                    r[rn] = address;
                }
            }
            0xea00..=0xea9f
                if rt < 8
                    && rm < 15
                    && (rn != 15 || matches!((high >> 5) & 7, 2 | 3))
                    && (rd != 15 || (high & 0x10 != 0 && matches!((high >> 5) & 7, 0 | 4))) =>
            {
                let shift = ((low >> 10) & 0x1c) | ((low >> 6) & 3);
                let (operand, carry) = self.shift_immediate(r[rm], (low >> 4) & 3, shift);
                let lhs = if rn == 15 { 0 } else { r[rn] };
                let result = match (high >> 5) & 7 {
                    0 => lhs & operand,
                    1 => lhs & !operand,
                    2 => lhs | operand,
                    3 => lhs | !operand,
                    _ => lhs ^ operand,
                };
                if rd != 15 {
                    r[rd] = result;
                }
                if high & 0x10 != 0 {
                    self.nz(result);
                    self.carry = carry;
                }
            }
            0xeb40..=0xeb7e if rt < 8 && rd < 15 && rm < 15 => {
                let shift = ((low >> 10) & 0x1c) | ((low >> 6) & 3);
                let operand = self.shift_immediate(r[rm], (low >> 4) & 3, shift).0;
                let flags = self.apsr();
                let operation = if high & 0x20 == 0 { 5 } else { 6 };
                r[rd] = self.data_processing(operation, r[rn], operand).unwrap();
                if high & 0x10 == 0 {
                    self.set_apsr(flags);
                }
            }
            0xeb00..=0xeb1e | 0xeba0..=0xebbe | 0xebc0..=0xebde
                if rt < 8 && rm < 15 && (rd < 15 || high & 0x10 != 0) =>
            {
                let shift = ((low >> 10) & 0x1c) | ((low >> 6) & 3);
                let operand = self.shift_immediate(r[rm], (low >> 4) & 3, shift).0;
                let subtract = high & 0x80 != 0;
                let (lhs, operand) = if high & 0x00e0 == 0x00c0 {
                    (operand, r[rn])
                } else {
                    (r[rn], operand)
                };
                let result = if high & 0x10 != 0 {
                    self.arithmetic(lhs, operand, subtract)
                } else if subtract {
                    lhs.wrapping_sub(operand)
                } else {
                    lhs.wrapping_add(operand)
                };
                if rd < 15 {
                    r[rd] = result;
                }
            }
            0xfa00..=0xfa1f | 0xfa40..=0xfa5f
                if rn != 13
                    && rt == 15
                    && low & 0xc0 == 0x80
                    && !matches!(rd, 13 | 15)
                    && !matches!(rm, 13 | 15) =>
            {
                let value = r[rm].rotate_right(((low >> 4) & 3) * 8);
                let extended = match high & 0x50 {
                    0x00 => (value as i16 as i32) as u32,
                    0x10 => value & 0xffff,
                    0x40 => (value as i8 as i32) as u32,
                    _ => value & 0xff,
                };
                r[rd] = if rn == 15 {
                    extended
                } else {
                    r[rn].wrapping_add(extended)
                };
            }
            0xfab0..=0xfabe
                if rn == rm && rt == 15 && low & 0xf0 == 0x80 && !matches!(rd, 13 | 15) =>
            {
                r[rd] = r[rm].leading_zeros();
            }
            0xfa00..=0xfa7e
                if rn != 13
                    && rt == 15
                    && low & 0xf0 == 0
                    && !matches!(rd, 13 | 15)
                    && !matches!(rm, 13 | 15) =>
            {
                let operation = [2, 3, 4, 7][((high >> 5) & 3) as usize];
                let flags = self.apsr();
                r[rd] = self.data_processing(operation, r[rn], r[rm]).unwrap();
                if high & 0x10 == 0 {
                    self.set_apsr(flags);
                }
            }
            0xfb00..=0xfb0e
                if rn != 13
                    && matches!(low & 0xf0, 0 | 0x10)
                    && !matches!(rm, 13 | 15)
                    && !matches!(rd, 13 | 15)
                    && rt != 13
                    && (low & 0x10 == 0 || rt != 15) =>
            {
                let product = r[rn].wrapping_mul(r[rm]);
                let addend = if rt == 15 { 0 } else { r[rt] };
                r[rd] = if low & 0x10 == 0 {
                    addend.wrapping_add(product)
                } else {
                    addend.wrapping_sub(product)
                };
            }
            0xfb10..=0xfb1e
                if low & 0xc0 == 0
                    && rn != 13
                    && rt != 13
                    && !matches!(rd, 13 | 15)
                    && !matches!(rm, 13 | 15) =>
            {
                let a = (r[rn] >> if low & 0x20 == 0 { 0 } else { 16 }) as i16;
                let b = (r[rm] >> if low & 0x10 == 0 { 0 } else { 16 }) as i16;
                let product = i32::from(a) * i32::from(b);
                if rt == 15 {
                    r[rd] = product as u32;
                } else {
                    let (sum, overflow) = product.overflowing_add(r[rt] as i32);
                    r[rd] = sum as u32;
                    self.saturation |= overflow;
                }
            }
            0xfb80..=0xfb8e | 0xfba0..=0xfbae | 0xfbc0..=0xfbce | 0xfbe0..=0xfbee
                if rn != 13
                    && low & 0xf0 == 0
                    && !matches!(rm, 13 | 15)
                    && !matches!(rt, 13 | 15)
                    && !matches!(rd, 13 | 15)
                    && rt != rd =>
            {
                let product = if high & 0x20 == 0 {
                    (i64::from(r[rn] as i32) * i64::from(r[rm] as i32)) as u64
                } else {
                    u64::from(r[rn]) * u64::from(r[rm])
                };
                let value = if high & 0x40 != 0 {
                    product.wrapping_add((u64::from(r[rd]) << 32) | u64::from(r[rt]))
                } else {
                    product
                };
                r[rt] = value as u32;
                r[rd] = (value >> 32) as u32;
            }
            0xf8df | 0xf85f if rt < 15 => {
                let base = *next & !3;
                let offset = low & 0xfff;
                let address = if high == 0xf8df {
                    base.wrapping_add(offset)
                } else {
                    base.wrapping_sub(offset)
                };
                r[rt] = bus.thumb_read::<4>(address)?;
            }
            0xfb90..=0xfb9e | 0xfbb0..=0xfbbe
                if rn != 13
                    && rt == 15
                    && low & 0xf0 == 0xf0
                    && !matches!(rd, 13 | 15)
                    && !matches!(rm, 13 | 15) =>
            {
                if r[rm] == 0 {
                    if bus.thumb_div_zero_trap() {
                        return Err("Cortex-M division by zero".into());
                    }
                    r[rd] = 0;
                } else if high & 0x20 != 0 {
                    r[rd] = r[rn] / r[rm];
                } else {
                    r[rd] = (r[rn] as i32).wrapping_div(r[rm] as i32) as u32;
                }
            }
            0xf990..=0xf99e | 0xf9b0..=0xf9be if rt < 15 => {
                let address = r[rn].wrapping_add(low & 0xfff);
                r[rt] = if high & 0x20 == 0 {
                    (bus.thumb_read::<1>(address)? as i8 as i32) as u32
                } else {
                    (bus.thumb_read::<2>(address)? as i16 as i32) as u32
                };
            }
            0xf880..=0xf8df if rn < 15 && rt < 15 => {
                let address = r[rn].wrapping_add(low & 0xfff);
                let width = 1 << ((high >> 5) & 3);
                if high & 0x10 != 0 {
                    r[rt] = match width {
                        1 => bus.thumb_read::<1>(address),
                        2 => bus.thumb_read::<2>(address),
                        _ => bus.thumb_read::<4>(address),
                    }
                    .map_err(|error| error.to_string())?;
                } else {
                    bus.thumb_write(address, width, r[rt], output)?;
                }
            }
            0xf85d if low == 0xfb04 => {
                let target = bus.thumb_read::<4>(r[13])?;
                r[13] = r[13].wrapping_add(4);
                *next = self.branch(target, r, bus)?;
            }
            0xf850..=0xf85e if rt == 15 && low & 0x0fc0 == 0 && !matches!(rm, 13 | 15) => {
                let address = r[rn].wrapping_add(r[rm] << ((low >> 4) & 3));
                *next = self.branch(bus.thumb_read::<4>(address)?, r, bus)?;
            }
            0xf800..=0xf85e | 0xf910..=0xf91e | 0xf930..=0xf93e
                if high & 15 != 15
                    && rt < 15
                    && (low & 0x0fc0 == 0 || (low & 0x800 != 0 && low & 0x500 != 0)) =>
            {
                let width = 1 << ((high >> 5) & 3);
                let writeback = low & 0x100 != 0;
                if writeback && rn == rt {
                    return Err(unsupported());
                }
                let register_offset = low & 0x800 == 0;
                if register_offset && rm == 15 {
                    return Err(unsupported());
                }
                let offset = if register_offset {
                    r[rm] << ((low >> 4) & 3)
                } else {
                    low & 0xff
                };
                let indexed = if !register_offset && low & 0x200 == 0 {
                    r[rn].wrapping_sub(offset)
                } else {
                    r[rn].wrapping_add(offset)
                };
                let address = if register_offset || low & 0x400 != 0 {
                    indexed
                } else {
                    r[rn]
                };
                if high & 0x10 == 0 {
                    bus.thumb_write(address, width, r[rt], output)?;
                } else {
                    let data = match width {
                        1 => bus.thumb_read::<1>(address),
                        2 => bus.thumb_read::<2>(address),
                        _ => bus.thumb_read::<4>(address),
                    }?;
                    r[rt] = match (high & 0x100 != 0, width) {
                        (true, 1) => (data as i8 as i32) as u32,
                        (true, 2) => (data as i16 as i32) as u32,
                        _ => data,
                    };
                }
                if writeback {
                    r[rn] = indexed;
                }
            }
            _ => return Err(unsupported()),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unaligned_thumb_data_is_limited_to_ram_and_respects_ccr() {
        let mut bus = Bus::default();
        let ram = 0x2000_0000;
        bus.map_ram(ram, 8).unwrap();
        bus.map_nvic(0xe0, None, true).unwrap();
        let mut output = Vec::new();
        bus.thumb_write(ram + 1, 4, 0x1234_abcd, &mut output)
            .unwrap();
        assert_eq!(bus.thumb_read::<2>(ram + 1).unwrap(), 0xabcd);
        assert!(bus.thumb_read::<4>(ram + 5).is_err());
        assert!(bus.thumb_write(0xe000_ed15, 2, 1, &mut output).is_err());
        bus.thumb_write(0xe000_ed14, 4, 0x208, &mut output).unwrap();
        assert!(bus.thumb_read::<2>(ram + 1).is_err());
    }

    #[test]
    fn table_branches_use_unaligned_architectural_pc_and_scaled_index() {
        let mut bus = Bus::default();
        bus.map_ram(0, 32).unwrap();
        let mut r = [0; 32];
        let mut thumb = Thumb::new(Some(0));
        thumb.thumb2 = true;
        let mut output = Vec::new();
        // TBB [pc, r0] at address 2, with the table beginning at address 6.
        bus.memory(2, 6)
            .unwrap()
            .copy_from_slice(&[0xdf, 0xe8, 0x00, 0xf0, 1, 3]);
        r[0] = 1;
        let mut pc = 2;
        thumb.step(&mut pc, &mut r, &mut bus, &mut output).unwrap();
        assert_eq!(pc, 12);

        // TBH [r1, r0, LSL #1] at address 12, with the second halfword selected.
        bus.memory(12, 4)
            .unwrap()
            .copy_from_slice(&[0xd1, 0xe8, 0x10, 0xf0]);
        bus.memory(20, 4).unwrap().copy_from_slice(&[1, 0, 6, 0]);
        r[1] = 20;
        thumb.step(&mut pc, &mut r, &mut bus, &mut output).unwrap();
        assert_eq!(pc, 28);
    }

    #[test]
    fn wide_branch_preserves_lr_and_runs_only_on_cortex_m4() {
        let mut bus = Bus::default();
        bus.map_ram(0, 16).unwrap();
        // b.w +4; nop; nop; mov.w r1, #0xaa
        bus.memory(0, 12).unwrap().copy_from_slice(&[
            0x00, 0xf0, 0x02, 0xb8, 0x00, 0xbf, 0x00, 0xbf, 0x4f, 0xf0, 0xaa, 0x01,
        ]);
        let mut pc = 0;
        let mut registers = [0; 32];
        registers[14] = 0x101;
        let mut output = Vec::new();
        let mut thumb = Thumb::new(Some(0));
        assert!(
            thumb
                .step(&mut pc, &mut registers, &mut bus, &mut output)
                .is_err()
        );
        thumb.thumb2 = true;
        thumb
            .step(&mut pc, &mut registers, &mut bus, &mut output)
            .unwrap();
        assert_eq!((pc, registers[14]), (8, 0x101));
        thumb
            .step(&mut pc, &mut registers, &mut bus, &mut output)
            .unwrap();
        assert_eq!((pc, registers[1]), (12, 0xaa));
    }
}

use std::io::Write;

use crate::bus::Bus;

use super::Thumb;

#[derive(Clone, Copy)]
pub(super) enum VectorTransfer {
    Interleaved {
        count: usize,
        beat: usize,
        width: usize,
    },
    Contiguous {
        width: usize,
        offset: u32,
        add: bool,
    },
    GatherScatter {
        width: usize,
        offsets: usize,
        scaled: bool,
    },
    Fpscr {
        offset: u32,
        add: bool,
    },
}

impl VectorTransfer {
    pub(super) fn decode(high: u32, low: u32) -> Option<Self> {
        if high & 0xffc0 == 0xfc80 && low & 0x1e1e == 0x1e00 {
            let count = if low & 1 == 0 { 2 } else { 4 };
            let beat = ((low >> 5) & 3) as usize;
            let size = ((low >> 7) & 3) as usize;
            return (size < 3 && beat < count).then_some(Self::Interleaved {
                count,
                beat,
                width: 1 << size,
            });
        }
        if high & 0xff60 == 0xed00 && low & 0xff80 == 0x2f80 {
            return Some(Self::Fpscr {
                offset: (low & 0x7f) * 4,
                add: high & 0x80 != 0,
            });
        }
        if high & 0xee00 != 0xec00 || low & 0x0f00 != 0x0f00 {
            return None;
        }
        if high & 0x100 == 0 && high & 0x80 != 0 && high & 0x20 == 0 && low & 0x1020 == 0 {
            let width = match low & 0xf0 {
                0x00 => 1,
                0x10 => 2,
                0x40 => 4,
                0xd0 => 8,
                _ => return None,
            };
            return Some(Self::GatherScatter {
                width,
                offsets: ((low >> 1) & 7) as usize,
                scaled: low & 1 != 0,
            });
        }
        if high & 0x100 != 0 || high & 0x20 != 0 {
            let width = if low & 0x1000 != 0 {
                4
            } else if high & 8 != 0 {
                2
            } else {
                1
            };
            return Some(Self::Contiguous {
                width,
                offset: (low & 0x7f) * width as u32,
                add: high & 0x80 != 0,
            });
        }
        None
    }
}

fn offset_address(base: u32, offset: u32, add: bool) -> u32 {
    if add {
        base.wrapping_add(offset)
    } else {
        base.wrapping_sub(offset)
    }
}

fn read_element(bus: &mut Bus, address: u32, width: usize, signed: bool) -> Result<u32, String> {
    match width {
        1 => bus
            .thumb_read::<1>(address)
            .map(|v| if signed { v as i8 as i32 as u32 } else { v }),
        2 => bus
            .thumb_read::<2>(address)
            .map(|v| if signed { v as i16 as i32 as u32 } else { v }),
        4 => bus.thumb_read::<4>(address),
        _ => unreachable!("vector bus beats are at most four bytes"),
    }
}

impl Thumb {
    pub(super) fn execute_vector_transfer(
        &mut self,
        transfer: VectorTransfer,
        high: u32,
        low: u32,
        r: &mut [u32; 32],
        bus: &mut Bus,
        output: &mut impl Write,
    ) -> Result<(), String> {
        let load = high & 0x10 != 0;
        let writeback = high & 0x20 != 0;
        let q = (low >> 13) as usize;
        let mut rn = (high & 15) as usize;
        if matches!(transfer, VectorTransfer::Contiguous { width: 1 | 2, .. }) {
            rn &= 7;
        }
        if rn == 15 {
            return Err("MVE memory transfer cannot use PC as its base register".into());
        }
        self.fp_active = true;
        let base = r[rn];
        let mut fp = self.fp;
        let mut updated_base = base;
        match transfer {
            VectorTransfer::Interleaved { count, beat, width } => {
                if q + count > 8 {
                    return Err("MVE structure transfer register list exceeds Q7".into());
                }
                let lanes = 16 / (count * width);
                for lane in 0..lanes {
                    for member in 0..count {
                        let address =
                            base.wrapping_add((beat * 16 + (lane * count + member) * width) as u32);
                        let byte = (beat * lanes + lane) * width;
                        let word = (q + member) * 4 + byte / 4;
                        let shift = (byte % 4) * 8;
                        if load {
                            let mask = u32::MAX >> (32 - width * 8) << shift;
                            let value = read_element(bus, address, width, false)?;
                            fp[word] = (fp[word] & !mask) | (value << shift);
                        } else {
                            bus.thumb_write(address, width, fp[word] >> shift, output)?;
                        }
                    }
                }
                updated_base = base.wrapping_add((16 * count) as u32);
            }
            VectorTransfer::Contiguous { width, offset, add } => {
                updated_base = offset_address(base, offset, add);
                let address = if high & 0x100 != 0 {
                    updated_base
                } else {
                    base
                };
                for lane in 0..4 {
                    let address = address.wrapping_add((lane * width) as u32);
                    if load {
                        fp[q * 4 + lane] = read_element(bus, address, width, high & 0x1000 == 0)?;
                    } else {
                        bus.thumb_write(address, width, fp[q * 4 + lane], output)?;
                    }
                }
            }
            VectorTransfer::GatherScatter {
                width,
                offsets,
                scaled,
            } => {
                let words = if width == 8 { 2 } else { 1 };
                for lane in (0..4).step_by(words) {
                    let offset = fp[offsets * 4 + lane];
                    let offset = if scaled {
                        offset.wrapping_mul(width as u32)
                    } else {
                        offset
                    };
                    for word in 0..words {
                        let address = base.wrapping_add(offset).wrapping_add(4 * word as u32);
                        let index = q * 4 + lane + word;
                        if load {
                            fp[index] =
                                read_element(bus, address, width.min(4), high & 0x1000 == 0)?;
                        } else {
                            bus.thumb_write(address, width.min(4), fp[index], output)?;
                        }
                    }
                }
            }
            VectorTransfer::Fpscr { offset, add } => {
                let address = offset_address(base, offset, add);
                if load {
                    self.fpscr = bus.thumb_read::<4>(address)?;
                } else {
                    bus.thumb_write(address, 4, self.fpscr, output)?;
                }
            }
        }
        self.fp = fp;
        if writeback {
            r[rn] = updated_base;
        }
        Ok(())
    }
}

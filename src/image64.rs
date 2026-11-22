use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    ops::Range,
};

use crate::{
    machine::Machine,
    rv64::{Isa, Rv64},
};

pub fn load(machine: &mut Machine, mut file: File, length: u64) -> Result<(), String> {
    let mut header = [0; 64];
    read_at(&mut file, 0, &mut header)?;
    if header[..7] != *b"\x7fELF\x02\x01\x01"
        || half(&header, 16) != 2
        || half(&header, 18) != 243
        || word(&header, 20) != 1
        || half(&header, 52) != 64
        || half(&header, 54) != 56
    {
        return Err("expected a little-endian RISC-V ELF64 executable".into());
    }
    let flags = word(&header, 48);
    // The serialized hart scheduler also satisfies an ELF requiring RVTSO.
    if flags & !0x17 != 0 || flags & 6 == 6 {
        return Err(format!("unsupported RISC-V ELF64 ISA flags {flags:#x}"));
    }
    let compressed = flags & 1 != 0;
    let default_isa = if let Some(cpu) = &machine.primary.rv64 {
        let isa = cpu.isa();
        if compressed && !isa.compressed {
            return Err("ELF requires the RV64 compressed extension".into());
        }
        // A shared image may start both integer-only and floating-point harts.
        let supported = |isa: Isa| match flags & 6 {
            2 => isa.single,
            4 => isa.double,
            _ => true,
        };
        if !(0..machine.hart_count())
            .any(|hart| supported(machine.hart_mut(hart).rv64.as_ref().unwrap().isa()))
        {
            return Err("RV64 platform lacks the ELF floating-point ABI extension".into());
        }
        None
    } else if machine.primary.name.is_some() || machine.is_cortex_m() {
        return Err("RISC-V ELF64 requires an RV64 platform".into());
    } else {
        Some(Isa::elf(compressed))
    };

    let entry = double(&header, 24);
    let table = double(&header, 32);
    let count = u64::from(half(&header, 56));
    if table < 64 || count == 0 || count == 0xffff || !file_range(table, count * 56, length) {
        return Err("invalid ELF64 program header table".into());
    }

    let mut executable = false;
    let mut segments = Vec::new();
    let minimum = if machine
        .primary
        .rv64
        .as_ref()
        .map_or(compressed, |cpu| cpu.isa().compressed)
    {
        2
    } else {
        4
    };
    if !entry.is_multiple_of(minimum) {
        return Err(format!("misaligned ELF64 entry {entry:#018x}"));
    }
    for index in 0..count {
        let mut segment = [0; 56];
        read_at(&mut file, table + index * 56, &mut segment)?;
        match word(&segment, 0) {
            2 | 3 => return Err("dynamic ELF executables are unsupported".into()),
            1 => {}
            _ => continue,
        }
        let offset = double(&segment, 8);
        let address = double(&segment, 24);
        let file_size = double(&segment, 32);
        let memory_size = double(&segment, 40);
        if file_size > memory_size || !file_range(offset, file_size, length) {
            return Err(format!("invalid ELF64 segment {index} size"));
        }
        if memory_size == 0 {
            continue;
        }
        let size = usize::try_from(memory_size).map_err(|_| "ELF64 segment exceeds host limits")?;
        let (backing, start) = machine
            .bus
            .ram_location64(address, size)
            .ok_or_else(|| format!("unmapped ELF64 segment at {address:#018x} ({size} bytes)"))?;
        segments.push(Segment {
            offset,
            address,
            file_size: file_size as usize,
            size,
            backing,
            start,
        });
        executable |= word(&segment, 4) & 1 != 0
            && entry
                .checked_sub(address)
                .is_some_and(|offset| file_range(offset, minimum, file_size));
    }
    if !executable {
        return Err("ELF64 entry is outside executable file-backed segments".into());
    }

    let mut initialized: Vec<(usize, Range<usize>)> = Vec::new();
    for segment in segments {
        let destination = machine
            .bus
            .memory64(segment.address, segment.size)
            .expect("validated ELF64 memory mapping");
        let (data, bss) = destination.split_at_mut(segment.file_size);
        read_at(&mut file, segment.offset, data)?;
        let zero_start = segment.start + segment.file_size;
        let end = segment.start + segment.size;
        if zero_start < end {
            // The backing starts demand-zero. Touch BSS only where previous
            // segments wrote bytes, including through another RAM alias.
            for (backing, range) in &initialized {
                if *backing == segment.backing {
                    let start = range.start.max(zero_start);
                    let end = range.end.min(end);
                    if start < end {
                        bss[start - zero_start..end - zero_start].fill(0);
                    }
                }
            }
        }
        if !data.is_empty() {
            initialized.push((segment.backing, segment.start..zero_start));
        }
    }
    if let Some(isa) = default_isa {
        machine.primary.rv64 = Some(Box::new(Rv64::new(isa)));
    }
    machine.set_rv64_entry(entry);
    Ok(())
}

struct Segment {
    address: u64,
    offset: u64,
    size: usize,
    file_size: usize,
    backing: usize,
    start: usize,
}

fn file_range(offset: u64, size: u64, length: u64) -> bool {
    offset.checked_add(size).is_some_and(|end| end <= length)
}

fn read_at(file: &mut File, offset: u64, bytes: &mut [u8]) -> Result<(), String> {
    file.seek(SeekFrom::Start(offset))
        .and_then(|_| file.read_exact(bytes))
        .map_err(|e| format!("ELF64 offset {offset:#x}: {e}"))
}

fn half(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}

fn word(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn double(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

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
    if half(&header, 16) != 2
        || half(&header, 18) != 243
        || word(&header, 20) != 1
        || half(&header, 52) != 64
        || half(&header, 54) != 56
    {
        return Err("expected a little-endian RISC-V ELF64 executable".into());
    }
    let flags = word(&header, 48);
    if flags & !1 != 0 {
        return Err(format!("unsupported RISC-V ELF64 ISA flags {flags:#x}"));
    }
    let compressed = flags & 1 != 0;
    if let Some(cpu) = &machine.primary.rv64 {
        if compressed && !cpu.compressed() {
            return Err("ELF requires the RV64 compressed extension".into());
        }
    } else if machine.primary.name.is_some() || machine.is_cortex_m() {
        return Err("RISC-V ELF64 requires an RV64 platform".into());
    } else {
        machine.primary.rv64 = Some(Box::new(Rv64::new(Isa {
            compressed,
            ..Isa::IM
        })));
    }

    let entry = double(&header, 24);
    let table = double(&header, 32);
    let count = u64::from(half(&header, 56));
    if table < 64 || count == 0 || count == 0xffff || !file_range(table, count * 56, length) {
        return Err("invalid ELF64 program header table".into());
    }

    let mut executable = false;
    let mut initialized: Vec<Range<u64>> = Vec::new();
    let minimum = if compressed { 2 } else { 4 };
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
        let end = address
            .checked_add(memory_size)
            .ok_or("ELF64 segment address overflow")?;
        let size = usize::try_from(memory_size).map_err(|_| "ELF64 segment exceeds host limits")?;
        let destination = machine
            .bus
            .memory64(address, size)
            .ok_or_else(|| format!("unmapped ELF64 segment at {address:#018x} ({size} bytes)"))?;
        let (data, bss) = destination.split_at_mut(file_size as usize);
        read_at(&mut file, offset, data)?;
        let zero_start = address + file_size;
        if zero_start < end {
            for range in &initialized {
                let start = range.start.max(zero_start);
                let end = range.end.min(end);
                if start < end {
                    bss[(start - zero_start) as usize..(end - zero_start) as usize].fill(0);
                }
            }
        }
        if file_size != 0 {
            initialized.push(address..zero_start);
        }
        executable |= word(&segment, 4) & 1 != 0
            && entry
                .checked_sub(address)
                .is_some_and(|offset| file_range(offset, minimum, file_size));
    }
    if !executable {
        return Err("ELF64 entry is outside executable file-backed segments".into());
    }
    machine.set_rv64_entry(entry);
    Ok(())
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

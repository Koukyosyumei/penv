use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    ops::Range,
};

use crate::machine::Machine;

struct ArmSegment {
    file: Range<u64>,
    virtual_address: u32,
    physical_address: u32,
}

pub fn load_binary(machine: &mut Machine, mut file: File, address: u32) -> Result<(), String> {
    let length = file.metadata().map_err(|e| e.to_string())?.len();
    if length == 0 {
        return Err("empty raw image".into());
    }
    let size = usize::try_from(length).map_err(|_| "raw image too large")?;
    let memory = machine
        .bus
        .memory(address, size)
        .map_err(|error| error.to_string())?;
    file.read_exact(memory).map_err(|error| error.to_string())
}

pub fn load(
    machine: &mut Machine,
    mut file: File,
    raw: bool,
    script_virtual_addresses: Option<bool>,
) -> Result<(), String> {
    if raw {
        return load_binary(machine, file, machine.raw_load_address());
    }
    let cortex_m = machine.is_cortex_m();
    let length = file.metadata().map_err(|e| e.to_string())?.len();
    let mut header = [0; 52];
    file.read_exact(&mut header)
        .map_err(|e| format!("ELF header: {e}"))?;
    if header[..7] != *b"\x7fELF\x01\x01\x01"
        || half(&header, 16) != 2
        || word(&header, 20) != 1
        || half(&header, 40) != 52
        || half(&header, 42) != 32
    {
        return Err("expected a little-endian ELF32 executable".into());
    }

    let flags = word(&header, 36);
    // Renode loads firmware independently of its linker's floating-point ABI.
    // Any floating-point instructions still require an implemented ISA extension.
    let abi_flags = if script_virtual_addresses.is_some() {
        0x6
    } else {
        0
    };
    if cortex_m {
        if half(&header, 18) != 40 || flags >> 24 != 5 || flags & 0x400 != 0 {
            return Err("expected an ARM EABI5 ELF32 executable without the hard-float ABI".into());
        }
    } else if half(&header, 18) != 243
        || flags & !(abi_flags | u32::from(machine.primary.compressed_instructions())) != 0
    {
        return Err("expected a RISC-V ELF32 executable with supported ISA flags".into());
    }

    let entry = word(&header, 24);
    let table = u64::from(word(&header, 28));
    let count = u64::from(half(&header, 44));
    if table < 52 || count == 0 || count == 0xffff || table + count * 32 > length {
        return Err("invalid ELF program header table".into());
    }

    let minimum_instruction_size = if machine.primary.compressed_instructions() {
        2
    } else {
        4
    };
    let mut entry_is_executable = false;
    let mut arm_segments = Vec::new();
    let mut initialized_ranges: Vec<Range<u64>> = Vec::new();
    for index in 0..count {
        let mut segment = [0; 32];
        read_at(&mut file, table + index * 32, &mut segment)?;
        match word(&segment, 0) {
            2 | 3 => return Err("dynamic ELF executables are unsupported".into()),
            1 => {}
            _ => continue,
        }

        let offset = u64::from(word(&segment, 4));
        let virtual_address = word(&segment, 8);
        let address = if cortex_m || script_virtual_addresses == Some(false) {
            word(&segment, 12)
        } else {
            virtual_address
        };
        let file_size = word(&segment, 16);
        let memory_size = word(&segment, 20);
        if file_size > memory_size || offset + u64::from(file_size) > length {
            return Err(format!("invalid ELF segment {index} size"));
        }
        // A relocated segment's initialized bytes live at its load address.
        // Cortex-M startup code copies them and initializes BSS at runtime.
        let load_size = if address != virtual_address {
            file_size
        } else {
            memory_size
        };
        if load_size == 0 {
            continue;
        }
        if cortex_m && file_size != 0 {
            arm_segments.push(ArmSegment {
                file: offset..offset + u64::from(file_size),
                virtual_address,
                physical_address: address,
            });
        }

        let destination = machine
            .bus
            .memory(address, load_size as usize)
            .map_err(|error| error.to_string())?;
        let (initialized, zeroed) = destination.split_at_mut(file_size as usize);
        read_at(&mut file, offset, initialized)?;
        // RAM starts zeroed. Only clear BSS bytes that an earlier segment wrote.
        let start = u64::from(address);
        let zero_start = start + u64::from(file_size);
        let zero_end = start + u64::from(load_size);
        if zero_start < zero_end {
            for range in &initialized_ranges {
                let begin = range.start.max(zero_start);
                let end = range.end.min(zero_end);
                if begin < end {
                    zeroed[(begin - zero_start) as usize..(end - zero_start) as usize].fill(0);
                }
            }
        }
        if file_size != 0 {
            initialized_ranges.push(start..zero_start);
        }
        if word(&segment, 24) & 1 != 0
            && entry >= address
            && u64::from(entry - address) + minimum_instruction_size <= u64::from(file_size)
        {
            entry_is_executable = true;
        }
    }
    if cortex_m {
        let image_base = arm_segments
            .iter()
            .map(|segment| segment.physical_address)
            .min()
            .ok_or("ELF has no file-backed loadable data")?;
        let vectors =
            arm_vector_base(&mut file, &header, length, &arm_segments)?.unwrap_or(image_base);
        machine.infer_cortex_m_vectors(vectors);
        return Ok(());
    }
    if !entry_is_executable {
        return Err("ELF entry is outside executable file-backed segments".into());
    }
    if let Some(symbols) = machine.function_symbols.as_mut() {
        load_code_symbols(&mut file, &header, length, symbols)?;
    }
    machine.set_entry(entry);
    Ok(())
}

fn load_code_symbols(
    file: &mut File,
    header: &[u8; 52],
    length: u64,
    symbols: &mut std::collections::BTreeMap<u32, String>,
) -> Result<(), String> {
    let table = u64::from(word(header, 32));
    let count = usize::from(half(header, 48));
    if count == 0 {
        return Ok(());
    }
    if table < 52 || half(header, 46) != 40 || table + count as u64 * 40 > length {
        return Err("invalid ELF section header table".into());
    }
    let mut sections = vec![[0; 40]; count];
    for (index, section) in sections.iter_mut().enumerate() {
        read_at(file, table + index as u64 * 40, section)?;
    }
    let range = |section: &[u8; 40]| {
        let offset = u64::from(word(section, 16));
        let size = u64::from(word(section, 20));
        (offset + size <= length)
            .then_some(offset..offset + size)
            .ok_or("invalid ELF symbol or string table bounds")
    };
    for section in sections.iter().filter(|section| word(*section, 4) == 2) {
        let entries = range(section)?;
        if word(section, 36) != 16 || (entries.end - entries.start) % 16 != 0 {
            return Err("invalid ELF symbol entry size".into());
        }
        let strings = sections
            .get(word(section, 24) as usize)
            .filter(|section| word(*section, 4) == 3)
            .ok_or("invalid ELF symbol string table")?;
        let strings = range(strings)?;
        let mut names = vec![0; (strings.end - strings.start) as usize];
        read_at(file, strings.start, &mut names)?;
        for offset in entries.step_by(16) {
            let mut entry = [0; 16];
            read_at(file, offset, &mut entry)?;
            let index = usize::from(half(&entry, 14));
            if !matches!(entry[12] & 15, 0 | 2)
                || index == 0
                || !sections
                    .get(index)
                    .is_some_and(|section| word(section, 8) & 4 != 0)
            {
                continue;
            }
            let name = names
                .get(word(&entry, 0) as usize..)
                .and_then(|bytes| {
                    bytes
                        .iter()
                        .position(|byte| *byte == 0)
                        .map(|end| &bytes[..end])
                })
                .ok_or("invalid ELF symbol name")?;
            if let Ok(name) = std::str::from_utf8(name)
                && !name.is_empty()
                && !name.starts_with('$')
            {
                symbols
                    .entry(word(&entry, 4))
                    .or_insert_with(|| name.to_owned());
            }
        }
    }
    Ok(())
}

fn arm_vector_base(
    file: &mut File,
    header: &[u8; 52],
    length: u64,
    segments: &[ArmSegment],
) -> Result<Option<u32>, String> {
    let table = u64::from(word(header, 32));
    let count = u64::from(half(header, 48));
    if count == 0 {
        return Ok(None);
    }
    if table < 52 || half(header, 46) != 40 || table + count * 40 > length {
        return Err("invalid ARM ELF section header table".into());
    }
    let mut earliest = None;
    for index in 0..count {
        let mut section = [0; 40];
        read_at(file, table + index * 40, &mut section)?;
        if matches!(word(&section, 4), 0 | 8) || word(&section, 8) & 2 == 0 {
            continue;
        }
        let size = u64::from(word(&section, 20));
        if size == 0 {
            continue;
        }
        let offset = u64::from(word(&section, 16));
        let address = u64::from(word(&section, 12));
        for segment in segments {
            if offset < segment.file.start || offset + size > segment.file.end {
                continue;
            }
            let delta = offset - segment.file.start;
            if address == u64::from(segment.virtual_address) + delta
                && let Ok(physical) = u32::try_from(u64::from(segment.physical_address) + delta)
            {
                earliest = Some(earliest.map_or(physical, |base: u32| base.min(physical)));
            }
        }
    }
    Ok(earliest)
}

fn half(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}

fn word(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn read_at(file: &mut File, offset: u64, bytes: &mut [u8]) -> Result<(), String> {
    file.seek(SeekFrom::Start(offset))
        .and_then(|_| file.read_exact(bytes))
        .map_err(|e| format!("ELF offset {offset:#x}: {e}"))
}

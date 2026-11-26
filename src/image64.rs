use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    ops::Range,
};

use crate::{
    machine::Machine,
    rv64::{Isa, Rv64},
};

pub fn load(
    machine: &mut Machine,
    mut file: File,
    length: u64,
    renode_load: bool,
) -> Result<(), String> {
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

    let minimum = minimum_width(machine, compressed) as u64;
    let entry_location = machine.bus.ram_location64(entry, minimum as usize);
    let mut linked_entry = None;
    let mut physical_entry = None;
    let mut ambiguous_entry = false;
    let mut segments = Vec::new();
    let mut skipped = Vec::new();
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
        let virtual_address = double(&segment, 16);
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
        let mut ranges = Vec::new();
        if renode_load {
            ranges.extend(machine.bus.mapped_ram_ranges64().filter_map(|range| {
                let begin = range.start.max(address);
                let limit = range.end.min(end);
                (begin < limit).then_some(begin..limit)
            }));
            ranges.sort_unstable_by_key(|range| range.start);
        } else {
            ranges.push(address..end);
        }
        let mut previous = address;
        for range in ranges {
            if previous < range.start {
                skipped.push(previous..range.start);
            }
            previous = range.end;
            let size = usize::try_from(range.end - range.start)
                .map_err(|_| "ELF64 segment exceeds host limits")?;
            let (backing, start) =
                machine
                    .bus
                    .ram_location64(range.start, size)
                    .ok_or_else(|| {
                        format!(
                            "unmapped ELF64 segment at {:#018x} ({size} bytes)",
                            range.start
                        )
                    })?;
            let delta = range.start - address;
            segments.push(Segment {
                offset: offset + delta.min(file_size),
                address: range.start,
                file_size: file_size.saturating_sub(delta).min(size as u64) as usize,
                size,
                backing,
                start,
            });
        }
        if previous < end {
            skipped.push(previous..end);
        }
        if word(&segment, 4) & 1 == 0 {
            continue;
        }
        let contains_entry = |base| {
            entry
                .checked_sub(base)
                .filter(|&delta| file_range(delta, minimum, file_size))
        };
        if contains_entry(address).is_some() && entry_location.is_some() {
            linked_entry = Some(entry);
        }
        if let Some(delta) = contains_entry(virtual_address) {
            let candidate = address + delta;
            let Some(location) = machine.bus.ram_location64(candidate, minimum as usize) else {
                continue;
            };
            if entry_location == Some(location) {
                linked_entry = Some(entry);
            }
            if physical_entry.is_some_and(|old| old != candidate) {
                ambiguous_entry = true;
            }
            physical_entry = Some(candidate);
        }
    }
    let entry = match (linked_entry, physical_entry) {
        (Some(entry), _) => entry,
        (None, Some(entry)) if !ambiguous_entry && entry.is_multiple_of(minimum) => entry,
        (None, Some(_)) => {
            return Err("ELF64 entry does not resolve to a unique aligned physical address".into());
        }
        (None, None) => return Err("ELF64 entry is outside executable file-backed segments".into()),
    };

    for range in skipped {
        eprintln!(
            "penv: skipped unmapped ELF64 range {:#018x}..{:#018x}",
            range.start, range.end
        );
    }
    let reloading = machine.monitor_firmware.is_some();
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
        if reloading {
            bss.fill(0);
        } else if zero_start < end {
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

fn minimum_width(machine: &Machine, compressed: bool) -> usize {
    if machine
        .primary
        .rv64
        .as_ref()
        .map_or(compressed, |cpu| cpu.isa().compressed)
    {
        2
    } else {
        4
    }
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

pub fn symbol(bytes: &[u8], name: &str) -> Result<u64, String> {
    symbols(bytes, name)?.into_iter().next()
        .ok_or_else(|| format!("ELF64 symbol {name:?} not found"))
}

pub(crate) fn symbols(bytes: &[u8], name: &str) -> Result<Vec<u64>, String> {
    let header = bytes.get(..64).ok_or("short ELF64 symbol file")?;
    if header[..7] != *b"\x7fELF\x02\x01\x01" || half(header, 18) != 243 || half(header, 58) != 64 {
        return Err("expected a little-endian RISC-V ELF64 section table".into());
    }
    let slice = |offset: u64, size: u64| -> Option<&[u8]> {
        bytes.get(usize::try_from(offset).ok()?..usize::try_from(offset.checked_add(size)?).ok()?)
    };
    let sections = slice(double(header, 40), u64::from(half(header, 60)) * 64)
        .ok_or("invalid ELF64 section table")?;
    let mut addresses = Vec::new();
    for symtab in sections
        .chunks_exact(64)
        .filter(|section| word(section, 4) == 2)
    {
        if double(symtab, 56) != 24 {
            return Err("invalid ELF64 symbol size".into());
        }
        let index = word(symtab, 40) as usize;
        let strings = sections
            .chunks_exact(64)
            .nth(index)
            .ok_or("invalid ELF64 string-table index")?;
        if word(strings, 4) != 3 {
            return Err("expected ELF64 symbol strings".into());
        }
        let strings = slice(double(strings, 24), double(strings, 32))
            .ok_or("invalid ELF64 symbol strings")?;
        let symbols =
            slice(double(symtab, 24), double(symtab, 32)).ok_or("invalid ELF64 symbol table")?;
        for entry in symbols.chunks_exact(24) {
            if half(entry, 6) == 0 {
                continue;
            }
            if let Some(tail) = strings.get(word(entry, 0) as usize..)
                && tail.split(|&byte| byte == 0).next() == Some(name.as_bytes())
            {
                addresses.push(double(entry, 8));
            }
        }
    }
    Ok(addresses)
}

#[cfg(test)]
mod symbol_tests {
    use super::*;

    #[test]
    fn reads_defined_elf64_symbols_from_the_linked_string_table() {
        let mut elf = [0u8; 312];
        elf[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        elf[18..20].copy_from_slice(&243u16.to_le_bytes());
        elf[40..48].copy_from_slice(&64u64.to_le_bytes());
        elf[58..60].copy_from_slice(&64u16.to_le_bytes());
        elf[60..62].copy_from_slice(&3u16.to_le_bytes());
        let symtab = &mut elf[128..192];
        symtab[4..8].copy_from_slice(&2u32.to_le_bytes());
        symtab[24..32].copy_from_slice(&256u64.to_le_bytes());
        symtab[32..40].copy_from_slice(&48u64.to_le_bytes());
        symtab[40..44].copy_from_slice(&2u32.to_le_bytes());
        symtab[56..64].copy_from_slice(&24u64.to_le_bytes());
        let strings = &mut elf[192..256];
        strings[4..8].copy_from_slice(&3u32.to_le_bytes());
        strings[24..32].copy_from_slice(&304u64.to_le_bytes());
        strings[32..40].copy_from_slice(&8u64.to_le_bytes());
        let symbol_entry = &mut elf[280..304];
        symbol_entry[..4].copy_from_slice(&1u32.to_le_bytes());
        symbol_entry[6..8].copy_from_slice(&1u16.to_le_bytes());
        symbol_entry[8..16].copy_from_slice(&0x8020_0000u64.to_le_bytes());
        elf[304..312].copy_from_slice(b"\0_start\0");
        assert_eq!(symbol(&elf, "_start"), Ok(0x8020_0000));
        assert!(symbol(&elf, "_other").is_err());
        assert!(symbol(&elf[..290], "_start").is_err());
    }
}

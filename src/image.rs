use std::{
    fs::File,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    ops::Range,
};

use crate::machine::Machine;

struct ArmSegment {
    file: Range<u64>,
    virtual_address: u32,
    physical_address: u32,
}

pub fn load_binary(machine: &mut Machine, file: File, address: u64) -> Result<(), String> {
    load_binary_in_context(machine, file, address, None)
}

pub fn load_binary_in_context(
    machine: &mut Machine,
    mut file: File,
    address: u64,
    hart: Option<usize>,
) -> Result<(), String> {
    let length = file.metadata().map_err(|e| e.to_string())?.len();
    if length == 0 {
        return Err("empty raw image".into());
    }
    let size = usize::try_from(length).map_err(|_| "raw image too large")?;
    let memory = match hart {
        Some(hart) => machine.bus.cpu_memory64(hart, address, size),
        None => machine.bus.memory64(address, size),
    }
    .ok_or_else(|| format!("unmapped RAM access at {address:#010x} ({size} bytes)"))?;
    file.read_exact(memory).map_err(|error| error.to_string())
}

pub fn load_hex(machine: &mut Machine, file: File) -> Result<(), String> {
    if !machine.is_cortex_m() {
        return Err("Intel HEX boot requires a Cortex-M platform".into());
    }
    let base = load_hex_records(file, |address, data| {
        machine.bus.memory(address, data.len()).map_err(|error| error.to_string())?.copy_from_slice(data);
        Ok(())
    })?;
    machine.infer_cortex_m_vectors(base);
    Ok(())
}

pub(crate) fn load_hex_in_context(machine: &mut Machine, file: File, context: &str) -> Result<(), String> {
    let hart = machine.bus.scoped_cpu_index(context)?;
    let mut first_missing = None;
    load_hex_records(file, |address, data| {
        for (offset, byte) in data.iter().enumerate() {
            let address = address + offset as u32;
            if let Some(memory) = machine.bus.cpu_memory64(hart, u64::from(address), 1) {
                memory[0] = *byte;
            } else {
                first_missing.get_or_insert(address);
            }
        }
        Ok(())
    })?;
    if let Some(address) = first_missing {
        crate::monitor_log::record(2, &format!(
            "sysbus: Tried to access bytes at non-existing peripheral at 0x{address:X}."
        ));
    }
    Ok(())
}

fn load_hex_records(
    file: File, mut write: impl FnMut(u32, &[u8]) -> Result<(), String>,
) -> Result<u32, String> {
    // Each record encodes up to 255 data bytes, five metadata bytes, and CRLF.
    const MAX_LINE: usize = 1 + 2 * (5 + 255) + 2;
    let mut reader = BufReader::new(file);
    let mut line = Vec::with_capacity(MAX_LINE);
    let mut record = [0; 260];
    let mut address_base = 0;
    let mut image_base = None;
    let mut eof = false;
    let mut line_number = 0;
    loop {
        line.clear();
        let length = (&mut reader)
            .take(MAX_LINE as u64 + 1)
            .read_until(b'\n', &mut line)
            .map_err(|e| e.to_string())?;
        if length == 0 {
            break;
        }
        line_number += 1;
        let error = |reason: &str| format!("Intel HEX line {line_number}: {reason}");
        if length > MAX_LINE {
            return Err(error("record is too long"));
        }
        let mut text = line.as_slice();
        while let Some((last, rest)) = text.split_last() {
            if !last.is_ascii_whitespace() {
                break;
            }
            text = rest;
        }
        if text.is_empty() {
            continue;
        }
        if eof {
            return Err(error("record after end-of-file"));
        }
        let digits = text
            .strip_prefix(b":")
            .ok_or_else(|| error("missing record prefix"))?;
        if digits.len() < 10 || digits.len() > 2 * record.len() || !digits.len().is_multiple_of(2) {
            return Err(error("invalid record length"));
        }
        let bytes = &mut record[..digits.len() / 2];
        for (byte, pair) in bytes.iter_mut().zip(digits.as_chunks::<2>().0) {
            let high = char::from(pair[0])
                .to_digit(16)
                .ok_or_else(|| error("invalid hex digit"))?;
            let low = char::from(pair[1])
                .to_digit(16)
                .ok_or_else(|| error("invalid hex digit"))?;
            *byte = ((high << 4) | low) as u8;
        }
        let count = usize::from(bytes[0]);
        if bytes.len() != count + 5 {
            return Err(error("byte count does not match record length"));
        }
        if bytes.iter().copied().fold(0u8, u8::wrapping_add) != 0 {
            return Err(error("checksum mismatch"));
        }
        let offset = u16::from_be_bytes([bytes[1], bytes[2]]);
        let data = &bytes[4..4 + count];
        match (bytes[3], offset, data) {
            (0, _, []) => {}
            (0, _, data) => {
                let address = address_base + u32::from(offset);
                address
                    .checked_add((count - 1) as u32)
                    .ok_or_else(|| error("data address exceeds 32 bits"))?;
                write(address, data).map_err(|reason| error(&reason))?;
                image_base = Some(image_base.map_or(address, |base: u32| base.min(address)));
            }
            (1, 0, []) => eof = true,
            (2, 0, [high, low]) => address_base = u32::from(u16::from_be_bytes([*high, *low])) << 4,
            (4, 0, [high, low]) => {
                address_base = u32::from(u16::from_be_bytes([*high, *low])) << 16
            }
            // Cortex-M starts at the reset vector even when the HEX file supplies an entry.
            (3 | 5, 0, [_, _, _, _]) => {}
            (0..=5, _, _) => return Err(error("invalid address or size for record type")),
            _ => return Err(error("unsupported record type")),
        }
    }
    if !eof {
        return Err("Intel HEX image is missing an end-of-file record".into());
    }
    image_base.ok_or_else(|| "Intel HEX image has no data".into())
}

pub fn load_rv64_dtb(machine: &mut Machine, mut file: File, address: u64) -> Result<(), String> {
    if machine.primary.rv64.is_none() {
        return Err("--dtb requires an RV64 machine".into());
    }
    if address & 7 != 0 {
        return Err("DTB address must be 8-byte aligned".into());
    }
    let mut header = [0; 40];
    file.read_exact(&mut header)
        .map_err(|e| format!("DTB header: {e}"))?;
    let total_size = u32::from_be_bytes(header[4..8].try_into().unwrap());
    let length = file.metadata().map_err(|e| e.to_string())?.len();
    if header[..4] != [0xd0, 0x0d, 0xfe, 0xed] || total_size < 40 || u64::from(total_size) > length
    {
        return Err("invalid DTB magic or size".into());
    }
    file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    load_binary(machine, file, address)?;
    for hart in 0..machine.hart_count() {
        machine
            .hart_mut(hart)
            .rv64
            .as_mut()
            .expect("RV64 machine")
            .set_boot_arguments(hart, address);
    }
    Ok(())
}

pub fn load_script_binary(
    machine: &mut Machine,
    mut file: File,
    address: u32,
) -> Result<(), String> {
    if !machine.bus.permits_unmapped_binary_bytes() {
        return load_binary(machine, file, u64::from(address));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.is_empty() {
        return Err("empty raw image".into());
    }
    machine
        .bus
        .write_bytes(address, &bytes, &mut std::io::sink())
        .map_err(|error| error.to_string())
}

pub fn load(
    machine: &mut Machine,
    mut file: File,
    raw: bool,
    script_virtual_addresses: Option<bool>,
    renode_elf_load: bool,
) -> Result<(), String> {
    if raw && renode_elf_load {
        return Err("--renode-elf-load requires an RV64 ELF".into());
    }

    if raw {
        return load_binary(machine, file, u64::from(machine.raw_load_address()));
    }
    let cortex_m = machine.is_cortex_m();
    let aarch32 = machine.primary.arm32.is_some();
    let length = file.metadata().map_err(|e| e.to_string())?.len();
    let mut header = [0; 52];
    file.read_exact(&mut header)
        .map_err(|e| format!("ELF header: {e}"))?;
    if header[..7] == *b"\x7fELF\x02\x01\x01" {
        return crate::image64::load(machine, file, length, renode_elf_load);
    }
    if renode_elf_load {
        return Err("--renode-elf-load requires an RV64 ELF".into());
    }
    if machine.primary.rv64.is_some() {
        return Err("RV64 requires a little-endian RISC-V ELF64 executable".into());
    }
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
    if cortex_m || aarch32 {
        if half(&header, 18) != 40
            || flags >> 24 != 5
            || (cortex_m && flags & 0x400 != 0 && !machine.has_cortex_m4())
        {
            return Err("expected an ARM EABI5 ELF32 executable with a supported float ABI".into());
        }
    } else if half(&header, 18) != 243
        || flags
            & !(abi_flags
                | u32::from(machine.primary.compressed_instructions())
                | (u32::from(machine.primary.embedded_base()) << 3))
            != 0
    {
        return Err("expected a RISC-V ELF32 executable with supported ISA flags".into());
    }

    let entry = word(&header, 24);
    let entry_address = if aarch32 { entry & !1 } else { entry };
    let table = u64::from(word(&header, 28));
    let count = u64::from(half(&header, 44));
    if table < 52 || count == 0 || count == 0xffff || table + count * 32 > length {
        return Err("invalid ELF program header table".into());
    }

    let minimum_instruction_size = if (aarch32 && entry & 1 != 0) || (!aarch32 && machine.primary.compressed_instructions()) {
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

        let start = u64::from(address);
        let zero_start = start + u64::from(file_size);
        let zero_end = start + u64::from(load_size);
        if zero_end > 1 << 32 {
            return Err(format!("ELF segment {index} exceeds the address space"));
        }
        for span in machine.bus.image_ram_spans(start, zero_end)? {
            let destination = machine.bus.memory64(span.start, (span.end - span.start) as usize)
                .expect("validated RAM span");
            let initialized_end = span.end.min(zero_start);
            if span.start < initialized_end {
                read_at(&mut file, offset + span.start - start,
                    &mut destination[..(initialized_end - span.start) as usize])?;
            }
            // RAM begins zeroed. Clear BSS bytes that an earlier segment initialized.
            for range in &initialized_ranges {
                let begin = range.start.max(zero_start).max(span.start);
                let end = range.end.min(span.end);
                if begin < end {
                    destination[(begin - span.start) as usize..(end - span.start) as usize].fill(0);
                }
            }
        }
        if file_size != 0 {
            initialized_ranges.push(start..zero_start);
        }
        let executable_base = if aarch32 { virtual_address } else { address };
        if word(&segment, 24) & 1 != 0
            && entry_address >= executable_base
            && u64::from(entry_address - executable_base) + minimum_instruction_size <= u64::from(file_size)
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

pub fn find_symbols<'a>(mut file: File, wanted: &[&'a str]) -> Result<Vec<(&'a str, u32)>, String> {
    let length = file.metadata().map_err(|error| error.to_string())?.len();
    let mut header = [0; 52];
    read_at(&mut file, 0, &mut header)?;
    if header[..7] != *b"\x7fELF\x01\x01\x01" {
        return Err("expected a little-endian ELF32 image for symbol lookup".into());
    }
    let table = u64::from(word(&header, 32));
    let count = u64::from(half(&header, 48));
    if count == 0 {
        return Ok(Vec::new());
    }
    if table < 52 || half(&header, 46) != 40 || table + count * 40 > length {
        return Err("invalid ELF section table for symbol lookup".into());
    }
    let name_capacity = wanted.iter().map(|name| name.len()).max().unwrap_or(0) + 1;
    let mut name = vec![0; name_capacity];
    let mut found = Vec::new();
    for index in 0..count {
        let mut section = [0; 40];
        read_at(&mut file, table + index * 40, &mut section)?;
        if !matches!(word(&section, 4), 2 | 11) {
            continue;
        }
        let symbols = symbol_range(&section, length)?;
        let section_link = u64::from(word(&section, 24));
        if word(&section, 36) != 16
            || (symbols.end - symbols.start) % 16 != 0
            || section_link >= count
        {
            return Err("invalid ELF symbol table".into());
        }
        read_at(&mut file, table + section_link * 40, &mut section)?;
        if word(&section, 4) != 3 {
            return Err("invalid ELF symbol string table".into());
        }
        let strings = symbol_range(&section, length)?;
        let mut entries = [0; 16 * 128];
        let mut offset = symbols.start;
        while offset < symbols.end {
            let size = (symbols.end - offset).min(entries.len() as u64) as usize;
            read_at(&mut file, offset, &mut entries[..size])?;
            let (batch, _) = entries[..size].as_chunks::<16>();
            for symbol in batch {
                let name_offset = strings.start + u64::from(word(symbol, 0));
                if half(symbol, 14) == 0 || name_offset >= strings.end {
                    continue;
                }
                let name_size = (strings.end - name_offset).min(name_capacity as u64) as usize;
                read_at(&mut file, name_offset, &mut name[..name_size])?;
                if let Some(end) = name[..name_size].iter().position(|&byte| byte == 0)
                    && let Some(&requested) = wanted
                        .iter()
                        .find(|candidate| candidate.as_bytes() == &name[..end])
                {
                    found.push((requested, word(symbol, 4)));
                }
            }
            offset += size as u64;
        }
    }
    Ok(found)
}

fn symbol_range(section: &[u8; 40], file_length: u64) -> Result<Range<u64>, String> {
    let start = u64::from(word(section, 16));
    let end = start + u64::from(word(section, 20));
    if end > file_length {
        return Err("ELF symbol section extends past the image".into());
    }
    Ok(start..end)
}

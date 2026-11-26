use std::path::Path;

use crate::thumb::opcode_counter::OpcodeCounters;

use super::Isa;

pub(crate) const RESOURCE_NAMES: &[&str] = &[
    "Custom",
    "Pseudo",
    "Rv32a",
    "Rv32b",
    "Rv32c",
    "Rv32d",
    "Rv32d-zfh",
    "Rv32f",
    "Rv32h",
    "Rv32i",
    "Rv32k",
    "Rv32m",
    "Rv32q",
    "Rv32q-zfh",
    "Rv32zfh",
    "Rv64a",
    "Rv64b",
    "Rv64c",
    "Rv64d",
    "Rv64f",
    "Rv64h",
    "Rv64i",
    "Rv64k",
    "Rv64m",
    "Rv64q",
    "Rv64zfh",
    "Rvc",
    "Rvk",
    "Rvp",
    "Rvv",
    "Rvv-pseudo",
    "Svinval",
    "System",
];

pub(crate) enum Selection {
    Architecture,
    Extension(String),
    Resource(String),
}

fn extension_resources(extension: &str) -> Result<&'static [&'static str], String> {
    Ok(match extension.to_ascii_uppercase().as_str() {
        "I" => &["System", "Rv32i", "Rv64i"],
        "M" => &["Rv32m", "Rv64m"],
        "A" => &["Rv32a", "Rv64a"],
        "F" => &["Rv32f", "Rv64f"],
        "B" => &["Rv32b", "Rv64b"],
        "D" => &["Rv32d", "Rv64d"],
        "C" => &["Rvc", "Rv64c"],
        "V" => &["Rvv", "Rvv-pseudo"],
        "S" | "U" => &[],
        _ => {
            return Err(format!(
                "Opcode counting for the {extension} extension is unsupported"
            ));
        }
    })
}

pub(crate) fn install(
    counters: &mut OpcodeCounters,
    isa: Isa,
    root: &Path,
    selection: &Selection,
) -> Result<(), String> {
    let resources: Vec<_> = match selection {
        Selection::Architecture => [
            ("I", true),
            ("M", isa.multiply),
            ("A", isa.atomic),
            ("F", isa.single),
            (
                "B",
                isa.bitmanip.zba || isa.bitmanip.zbb || isa.bitmanip.zbs,
            ),
            ("D", isa.double),
            ("C", isa.compressed),
            ("V", isa.vector),
        ]
        .into_iter()
        .filter(|(_, enabled)| *enabled)
        .flat_map(|(extension, _)| extension_resources(extension).unwrap().iter().copied())
        .collect(),
        Selection::Extension(extension) => extension_resources(extension)?.to_vec(),
        Selection::Resource(resource) => {
            if !RESOURCE_NAMES.contains(&resource.as_str()) {
                return Err(format!("Unknown RISC-V opcode resource {resource:?}"));
            }
            vec![resource.as_str()]
        }
    };
    let directory = root.join("src/Infrastructure/src/Emulator/Cores/RiscV/opcodes");
    let mut pending = Vec::new();
    for resource in resources {
        let path = directory.join(format!("opcodes-{}", resource.to_ascii_lowercase()));
        let input = std::fs::read_to_string(&path).map_err(|error| {
            format!("reading RISC-V opcode resource {}: {error}", path.display())
        })?;
        for (number, line) in input.lines().enumerate() {
            let line = line.split('#').next().unwrap_or_default();
            let mut words = line.split_ascii_whitespace();
            let Some(name) = words.next() else { continue };
            let (mask, value) = parse_fields(words)
                .map_err(|error| format!("{}:{}: {error}", path.display(), number + 1))?;
            pending.push((name.to_owned(), mask, value));
        }
    }
    for (name, mask, value) in pending {
        counters.install_mask(&name, mask, value)?;
    }
    counters.enabled = true;
    Ok(())
}

fn parse_fields<'a>(words: impl Iterator<Item = &'a str>) -> Result<(u64, u64), String> {
    let mut mask = 0;
    let mut opcode = 0;
    for word in words {
        let Some((range, value)) = word.split_once('=') else {
            continue;
        };
        if value == "ignore" {
            continue;
        }
        let (high, low) = range.split_once("..").unwrap_or((range, range));
        let high: u32 = high
            .parse()
            .map_err(|_| format!("Invalid opcode field {range}"))?;
        let low: u32 = low
            .parse()
            .map_err(|_| format!("Invalid opcode field {range}"))?;
        if high >= 32 || low > high {
            return Err(format!("Invalid opcode field {range}"));
        }
        let width = high - low + 1;
        let field_mask = (1u64 << width) - 1;
        let value = if let Some(bits) = value.strip_prefix("0b") {
            u64::from_str_radix(bits, 2).map_err(|_| format!("Invalid opcode bits {value}"))?
        } else {
            crate::parse_address64(value)?
        };
        if value > field_mask {
            return Err(format!("Opcode value overflows field {range}"));
        }
        mask |= field_mask << low;
        opcode |= value << low;
    }
    Ok((mask, opcode))
}

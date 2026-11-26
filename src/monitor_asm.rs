//! RISC-V monitor assembly using the same LLVM assembler as the CLI.
use crate::machine::Machine;
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

pub(crate) fn handle(machine: &mut Machine, command: &str) -> Option<Result<usize, String>> {
    let command = command.strip_prefix("AssembleMonitorBlock ")?;
    Some(assemble(machine, command))
}

fn assemble(machine: &mut Machine, command: &str) -> Result<usize, String> {
    let words: Vec<_> = command.split_whitespace().collect();
    let [cpu, address, requested_triple, text] = words.as_slice() else {
        return Err("AssembleBlock requires a CPU, an address, and assembly".into());
    };
    let address = crate::parse_address64(address)?;
    if !text.is_ascii() || !text.len().is_multiple_of(2) || text.len() > 131_072 {
        return Err("invalid or oversized assembly source".into());
    }
    let bytes = (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).map_err(|error| error.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    let source = String::from_utf8(bytes).map_err(|error| error.to_string())?;
    let cpu = if *cpu == "-" {
        if machine.hart_count() != 1 {
            return Err("multiple CPUs require an explicit CPU index".into());
        }
        machine.monitor_assembly_cpu_name(0)?.to_owned()
    } else if let Some(index) = cpu.strip_prefix("index:") {
        let index = index.parse().map_err(|_| "invalid assembly CPU index")?;
        machine.monitor_assembly_cpu_name(index)?.to_owned()
    } else {
        (*cpu).to_owned()
    };
    assemble_source(machine, &cpu, address, requested_triple, &source)
}

fn matches_triple(requested: &str, target: &str) -> bool {
    requested == "default" || requested == target
        || target.strip_suffix("-none-eabi") == Some(requested)
        || requested == "thumb" && target.starts_with("thumb")
}

pub(crate) fn assemble_source(
    machine: &mut Machine,
    cpu: &str,
    address: u64,
    requested_triple: &str,
    source: &str,
) -> Result<usize, String> {
    if source.len() > 65_536 {
        return Err("oversized assembly source".into());
    }
    let (hart, triple, features) = machine.monitor_assembly_target(cpu)?;
    let thumb = triple.starts_with("thumb");
    let arm = thumb || triple.starts_with("arm");
    let mut cleaned = String::new();
    if arm {
        let mut rest = source;
        while let Some((before, comment)) = rest.split_once("/*") {
            cleaned.push_str(before);
            cleaned.push(' ');
            rest = comment.split_once("*/").ok_or("unterminated ARM assembly comment")?.1;
        }
        cleaned.push_str(rest);
        cleaned = cleaned.lines().map(|line| {
            let line = line.split_once("//").map_or(line, |(code, _)| code);
            line.split_once("  # ").map_or(line, |(code, _)| code)
        })
            .collect::<Vec<_>>().join("\n");
    }
    let source = if arm { cleaned.as_str() } else { source };
    let comment = if arm { '@' } else { '#' };
    let instructions: Vec<_> = source
        .lines()
        .map(|line| line.split_once(comment).map_or(line, |(code, _)| code))
        .flat_map(|line| line.split(';'))
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if instructions.is_empty()
        || instructions.iter().any(|line| {
            if let Some((label, instruction)) = line.split_once(':') {
                label.is_empty()
                    || !label
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
                    || unsupported_assembly_directive(instruction.trim_start(), arm)
                    || instruction.contains(':')
            } else {
                unsupported_assembly_directive(line, arm)
            }
        })
    {
        return Err("monitor assembly requires instructions and optional local labels".into());
    }
    if !matches_triple(requested_triple, triple) {
        return Err(format!(
            "assembler triple {requested_triple} is unavailable for {triple}"
        ));
    }
    let has_data = arm && instructions.iter().any(|line| {
        let instruction = line.split_once(':').map_or(*line, |(_, text)| text.trim_start());
        matches!(instruction.split_ascii_whitespace().next(), Some(".word" | ".short" | ".byte" | ".asciz" | ".align" | ".balign"))
    });
    let labels: std::collections::HashSet<_> = instructions.iter()
        .filter_map(|line| line.split_once(':').map(|(label, _)| label))
        .collect();
    // A mode directive at a label applies to the instructions at that
    // address. Mark Thumb entry labels so ARM BLX relocations interwork.
    let mut normalized = Vec::with_capacity(instructions.len());
    let mut instructions = instructions.into_iter().peekable();
    while let Some(line) = instructions.next() {
        if arm && line.ends_with(':') && instructions.peek() == Some(&".thumb") {
            instructions.next();
            normalized.extend([".thumb", ".thumb_func", line]);
        } else {
            normalized.push(line);
        }
    }
    let instructions = normalized
        .into_iter()
        .map(|line| {
            let line = if arm {
                lower_arm_symbol_move(line, &labels)
            } else {
                line.to_owned()
            };
            let line = normalize_vector_memory_operand(&line);
            let line = if features.split(',').any(|feature| feature == "+zicond") {
                lower_zicond_instruction(&line)
            } else {
                line
            };
            lower_andes_instruction(&line, triple)
        })
        .collect::<Result<Vec<_>, _>>()?
        .join(
            "
",
        )
        + "
";
    let output = crate::llvm_mc_for(triple, &features, "--show-encoding", &instructions)?;
    if !output.status.success() {
        return Err(format!(
            "llvm-mc {triple}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let output_text = String::from_utf8_lossy(&output.stdout);
    let image = match inline_encoding(&output_text)? {
        Some(image) if !has_data && !instructions.contains(".4byte ") && !instructions.contains(".insn ") && !output_text.contains("fixup ") => image,
        _ => link_instructions(triple, &features, address, &instructions)?,
    };
    if image.is_empty() {
        return Err("llvm-mc emitted no instructions".into());
    }
    let target = match hart {
        Some(hart) => machine.bus.cpu_memory64(hart, address, image.len()),
        None => machine.bus.memory64(address, image.len()),
    }
    .ok_or_else(|| format!("unmapped instruction memory at {cpu}@{address:#x}"))?;
    target.copy_from_slice(&image);
    Ok(image.len())
}

fn lower_arm_symbol_move(line: &str, labels: &std::collections::HashSet<&str>) -> String {
    let (label, instruction) = line.split_once(':').map_or(("", line), |(label, instruction)| {
        (&line[..label.len() + 1], instruction.trim_start())
    });
    if let Some((opcode, operands)) = instruction.split_once(char::is_whitespace)
        && matches!(opcode, "mov" | "mov.w")
        && let Some((register, symbol)) = operands.split_once(',')
        && labels.contains(symbol.trim())
    {
        return format!("{label} ldr {}, ={}", register.trim(), symbol.trim());
    }
    line.to_owned()
}

fn unsupported_assembly_directive(instruction: &str, thumb: bool) -> bool {
    if !instruction.starts_with('.') || instruction.starts_with(".insn r ") {
        return false;
    }
    if !thumb {
        return true;
    }
    let mut words = instruction.split_ascii_whitespace();
    match words.next() {
        Some(".word" | ".short" | ".byte" | ".asciz") => false,
        Some(".thumb" | ".arm") => words.next().is_some(),
        Some(".align") => !words.next().and_then(|word| word.parse::<u8>().ok())
            .is_some_and(|alignment| alignment <= 12 && words.next().is_none()),
        Some(".balign") => !words.next().and_then(|word| word.parse::<u32>().ok())
            .is_some_and(|alignment| alignment <= 4096 && alignment.is_power_of_two() && words.next().is_none()),
        _ => true,
    }
}

// Spell these using the architectural encoding for host LLVM versions that
// predate Zicond support. LLVM still validates the operands and resolves labels.
fn lower_zicond_instruction(line: &str) -> String {
    let (label, instruction) = line.split_once(':').map_or(("", line), |(label, rest)| {
        (&line[..label.len() + 1], rest.trim_start())
    });
    let Some((name, operands)) = instruction.split_once(char::is_whitespace) else {
        return line.to_owned();
    };
    let funct3 = match name.to_ascii_lowercase().as_str() {
        "czero.eqz" => 5,
        "czero.nez" => 7,
        _ => return line.to_owned(),
    };
    format!("{label} .insn r 0x33, {funct3}, 7, {operands}")
}

fn lower_andes_instruction(line: &str, triple: &str) -> Result<String, String> {
    let (label, instruction) = line.split_once(':').map_or(("", line), |(label, rest)| {
        (&line[..label.len() + 1], rest.trim_start())
    });
    let Some((name, args)) = instruction.split_once(char::is_whitespace) else {
        return Ok(line.into());
    };
    let funct7 = match name {
        "nds.lea.h" => Some(5),
        "nds.lea.w" => Some(6),
        "nds.lea.d" => Some(7),
        "nds.lea.b.ze" if triple == "riscv64" => Some(8),
        "nds.lea.h.ze" if triple == "riscv64" => Some(9),
        "nds.lea.w.ze" if triple == "riscv64" => Some(10),
        "nds.lea.d.ze" if triple == "riscv64" => Some(11),
        "nds.ffb" => Some(16),
        "nds.ffzmism" => Some(17),
        "nds.ffmism" => Some(18),
        "nds.flmism" => Some(19),
        _ => None,
    };
    if let Some(funct7) = funct7 {
        let args: Vec<_> = args.split(',').map(str::trim).collect();
        let [rd, rs1, rs2] = args.as_slice() else {
            return Err(format!("{name} requires three registers"));
        };
        let encoding = (funct7 << 25) | (andes_register(rs2)? << 20)
            | (andes_register(rs1)? << 15) | (andes_register(rd)? << 7) | 0x5b;
        return Ok(format!("{label} .4byte {encoding:#010x}"));
    }
    if let kind @ ("nds.bfoz" | "nds.bfos") = name {
        let args: Vec<_> = args.split(',').map(str::trim).collect();
        let [rd, rs1, msb, lsb] = args.as_slice() else {
            return Err(format!("{name} requires two registers and two bit indices"));
        };
        let (msb, lsb) = (crate::parse_address(msb)?, crate::parse_address(lsb)?);
        let width = if triple == "riscv32" { 32 } else { 64 };
        if msb >= width || lsb >= width {
            return Err(format!("{name} bit index must be below {width}"));
        }
        let funct3 = if kind == "nds.bfoz" { 2 } else { 3 };
        let encoding = (msb << 26) | (lsb << 20) | (andes_register(rs1)? << 15)
            | (funct3 << 12) | (andes_register(rd)? << 7) | 0x5b;
        return Ok(format!("{label} .4byte {encoding:#010x}"));
    }
    if matches!(name, "nds.vfpmadt.vf" | "nds.vfpmadb.vf") {
        let args: Vec<_> = args.split(',').map(str::trim).collect();
        let (rd, fs1, vs2, unmasked) = match args.as_slice() {
            [rd, fs1, vs2] => (rd, fs1, vs2, true),
            [rd, fs1, vs2, "v0.t"] => (rd, fs1, vs2, false),
            _ => return Err(format!("{name} requires a destination, float source, vector source and optional v0.t")),
        };
        let function = if name == "nds.vfpmadt.vf" { 2 } else { 3 };
        let encoding = (function << 26) | (u32::from(unmasked) << 25)
            | (andes_vector_register(vs2)? << 20) | (andes_float_register(fs1)? << 15)
            | 0x405b | (andes_vector_register(rd)? << 7);
        return Ok(format!("{label} .4byte {encoding:#010x}"));
    }
    let dot = match name {
        "nds.vd4dots.vv" => Some(4),
        "nds.vd4dotsu.vv" => Some(5),
        "nds.vd4dotu.vv" => Some(7),
        _ => None,
    };
    if let Some(function) = dot {
        let args: Vec<_> = args.split(',').map(str::trim).collect();
        let (rd, rs1, rs2, unmasked) = match args.as_slice() {
            [rd, rs1, rs2] => (rd, rs1, rs2, true),
            [rd, rs1, rs2, "v0.t"] => (rd, rs1, rs2, false),
            _ => return Err(format!("{name} requires three vector registers and optional v0.t")),
        };
        let encoding = (function << 26) | (u32::from(unmasked) << 25)
            | (andes_vector_register(rs2)? << 20) | (andes_vector_register(rs1)? << 15)
            | 0x405b | (andes_vector_register(rd)? << 7);
        return Ok(format!("{label} .4byte {encoding:#010x}"));
    }
    let integer_conversion = match name {
        "nds.vfwcvt.f.n.v" => Some(4),
        "nds.vfwcvt.f.nu.v" => Some(5),
        "nds.vfwcvt.f.b.v" => Some(6),
        "nds.vfwcvt.f.bu.v" => Some(7),
        _ => None,
    };
    if let Some(selector) = integer_conversion {
        let args: Vec<_> = args.split(',').map(str::trim).collect();
        let (rd, vs2, unmasked) = match args.as_slice() {
            [rd, vs2] => (rd, vs2, true),
            [rd, vs2, "v0.t"] => (rd, vs2, false),
            _ => return Err(format!("{name} requires two vector registers and optional v0.t")),
        };
        let encoding = (u32::from(unmasked) << 25) | (andes_vector_register(vs2)? << 20)
            | (selector << 15) | 0x405b | (andes_vector_register(rd)? << 7);
        return Ok(format!("{label} .4byte {encoding:#010x}"));
    }
    if matches!(name, "nds.vln8.v" | "nds.vlnu8.v" | "nds.vle4.v") {
        let args: Vec<_> = args.split(',').map(str::trim).collect();
        let (vd, base, unmasked) = match args.as_slice() {
            [vd, base] => (vd, base, true),
            [vd, base, "v0.t"] => (vd, base, false),
            _ => return Err(format!("{name} requires a vector register, a base address and optional v0.t")),
        };
        let base = base.strip_prefix('(').and_then(|base| base.strip_suffix(')'))
            .ok_or("expected a parenthesized Andes vector load base register")?;
        let encoding = if name == "nds.vle4.v" {
            if !unmasked {
                return Err("nds.vle4.v does not accept a mask".into());
            }
            0x0800_405b
        } else {
            0x0820_405b | (u32::from(name == "nds.vlnu8.v") << 20)
        };
        let encoding = encoding
            | (u32::from(unmasked) << 25) | (andes_register(base)? << 15)
            | (andes_vector_register(vd)? << 7);
        return Ok(format!("{label} .4byte {encoding:#010x}"));
    }
    if matches!(name, "nds.vfwcvt.s.bf16" | "nds.vfncvt.bf16.s") {
        let (rd, source) = args.split_once(',')
            .ok_or_else(|| format!("{name} requires two vector registers"))?;
        let rd = andes_vector_register(rd.trim())?;
        let source = andes_vector_register(source.trim())?;
        let narrow = u32::from(name == "nds.vfncvt.bf16.s");
        let encoding = 0x0000_405b | (narrow << 15) | (source << 20) | (rd << 7);
        return Ok(format!("{label} .4byte {encoding:#010x}"));
    }
    if matches!(name, "nds.fcvt.s.bf16" | "nds.fcvt.bf16.s") {
        let (rd, source) = args.split_once(',')
            .ok_or_else(|| format!("{name} requires two floating-point registers"))?;
        let rd = andes_float_register(rd.trim())?;
        let source = andes_float_register(source.trim())?;
        let narrow = u32::from(name == "nds.fcvt.bf16.s");
        let encoding = 0x0001_405b | (narrow << 15) | (source << 20) | (rd << 7);
        return Ok(format!("{label} .4byte {encoding:#010x}"));
    }
    let gp = match name {
        "nds.lbgp" => Some((0x0b, 0, 1, false)),
        "nds.addigp" => Some((0x0b, 1, 1, false)),
        "nds.lbugp" => Some((0x0b, 2, 1, false)),
        "nds.sbgp" if triple == "riscv32" => Some((0x0b, 3, 1, true)),
        "nds.shgp" if triple == "riscv32" => Some((0x2b, 0, 2, true)),
        "nds.lhgp" => Some((0x2b, 1, 2, false)),
        "nds.lwgp" => Some((0x2b, 2, 4, false)),
        "nds.swgp" if triple == "riscv32" => Some((0x2b, 4, 4, true)),
        "nds.lhugp" => Some((0x2b, 5, 2, false)),
        _ => None,
    };
    if let Some((opcode, funct3, width, store)) = gp {
        let (register, offset) = args.split_once(',')
            .ok_or_else(|| format!("{name} requires a register and a GP displacement"))?;
        let register = andes_register(register.trim())?;
        let offset = signed_assembly_immediate(offset.trim())?;
        let bits = if width == 4 { 19 } else { 18 };
        let limit = 1i64 << (bits - 1);
        if !(-limit..limit).contains(&offset) || offset % width != 0 {
            return Err(format!("{name} displacement must fit in {bits} signed bits and align to {width} bytes"));
        }
        let offset = offset as u32;
        let shared = (offset & 0x7000) << 5 | (offset & 0x18000);
        let payload = match (store, width) {
            (false, 4) => (offset & 0x7fc) << 20 | (offset & 0x800) << 9
                | (offset & 0x20000) << 4 | (offset & 0x40000) << 13,
            (false, _) => (offset & 0x7fe) << 20 | (offset & 0x800) << 9
                | (offset & 0x20000) << 14,
            (true, 4) => (offset & 0x1c) << 7 | (offset & 0x7e0) << 20
                | (offset & 0x800) >> 4 | (offset & 0x20000) >> 9 | (offset & 0x40000) << 13,
            (true, _) => (offset & 0x1e) << 7 | (offset & 0x7e0) << 20
                | (offset & 0x800) >> 4 | (offset & 0x20000) << 14,
        };
        let encoding = opcode | (funct3 << 12) | shared | payload
            | (register << if store { 20 } else { 7 })
            | (if width == 1 { (offset & 1) << 14 } else { 0 });
        return Ok(format!("{label} .4byte {encoding:#010x}"));
    }
    let (kind, test) = match name {
        "nds.bbc" => (7, false),
        "nds.bbs" => (7, true),
        "nds.beqc" if triple == "riscv32" => (5, false),
        "nds.bnec" if triple == "riscv32" => (6, false),
        _ => return Ok(line.into()),
    };
    let operands: Vec<_> = args.split(',').map(str::trim).collect();
    let [register, immediate, offset] = operands.as_slice() else {
        return Err(format!("{name} requires a register, an immediate and a byte displacement"));
    };
    let register = andes_register(register)?;
    let immediate = crate::parse_address(immediate)?;
    let limit = match (kind, triple) { (7, "riscv32") => 32, (7, _) => 64, _ => 128 };
    if immediate >= limit {
        return Err(format!("Andes branch immediate exceeds the range 0..{}", limit - 1));
    }
    let offset = signed_assembly_immediate(offset)?;
    if !(-1024..=1022).contains(&offset) || offset % 2 != 0 {
        return Err("Andes branch displacement must be even and between -1024 and 1022".into());
    }
    let offset = offset as u32;
    let bit30 = if kind == 7 { u32::from(test) } else { immediate >> 6 };
    let encoding = 0x5b | ((immediate & 32) << 2) | (kind << 12) | (register << 15)
        | ((immediate & 31) << 20) | (bit30 << 30)
        | ((offset & 0x400) << 21) | ((offset & 0x3e0) << 20) | ((offset & 0x1e) << 7);
    Ok(format!("{label} .4byte {encoding:#010x}"))
}

fn signed_assembly_immediate(value: &str) -> Result<i64, String> {
    if let Some(magnitude) = value.strip_prefix('-') {
        Ok(-i64::from(crate::parse_address(magnitude)?))
    } else {
        crate::parse_address(value).map(i64::from)
    }
}

fn andes_vector_register(register: &str) -> Result<u32, String> {
    register.strip_prefix('v').and_then(|index| index.parse::<u32>().ok())
        .filter(|&index| index < 32)
        .ok_or_else(|| format!("invalid vector register {register}"))
}

fn andes_float_register(register: &str) -> Result<u32, String> {
    register.strip_prefix('f')
        .and_then(|index| index.parse::<u32>().ok())
        .filter(|&index| index < 32)
        .ok_or_else(|| format!("invalid floating-point register {register}"))
}

fn andes_register(register: &str) -> Result<u32, String> {
    let registers = [
        "zero", "ra", "sp", "gp", "tp", "t0", "t1", "t2", "s0", "s1", "a0", "a1",
        "a2", "a3", "a4", "a5", "a6", "a7", "s2", "s3", "s4", "s5", "s6", "s7",
        "s8", "s9", "s10", "s11", "t3", "t4", "t5", "t6",
    ];
    registers.iter().position(|name| *name == register)
        .or_else(|| register.strip_prefix('x')?.parse::<usize>().ok())
        .or_else(|| (register == "fp").then_some(8))
        .filter(|register| *register < 32)
        .map(|index| index as u32)
        .ok_or_else(|| format!("invalid Andes register {register}"))
}

// The Renode assembler accepts explicit zero displacement on RVV memory
// operands. LLVM's RISC-V assembly syntax spells that operand as (base).
fn normalize_vector_memory_operand(line: &str) -> String {
    let (prefix, statement) = line.split_once(':').map_or(("", line), |(label, rest)| {
        (&line[..label.len() + 1], rest.trim_start())
    });
    if let Some((mnemonic, operands)) = statement.split_once(char::is_whitespace)
        && (mnemonic.starts_with("vl") || mnemonic.starts_with("vs"))
        && mnemonic.ends_with(".v")
        && let Some((first, remaining)) = operands.split_once(',')
        && let Some(base) = remaining.trim_start().strip_prefix("0(")
    {
        return format!("{prefix} {mnemonic} {first}, ({base}");
    }
    line.to_owned()
}

pub(crate) fn disassemble_source(
    machine: &mut Machine, cpu: &str, address: u64, length: usize, requested_triple: &str,
) -> Result<String, String> {
    let (hart, triple, features) = machine.monitor_assembly_target(cpu)?;
    if !matches_triple(requested_triple, triple)
    {
        return Err(format!("disassembler triple {requested_triple} is unavailable for {triple}"));
    }
    disassemble_bus(&mut machine.bus, hart, triple, &features, address, length)
}

pub(crate) fn disassemble_bus(
    bus: &mut crate::bus::Bus, hart: Option<usize>, triple: &str, features: &str,
    address: u64, length: usize,
) -> Result<String, String> {
    use std::fmt::Write;

    let mut input = String::with_capacity(5 * length + 1);
    let mut bytes = Vec::with_capacity(length);
    if let Some(bytes_in_ram) = match hart {
        Some(hart) => bus.cpu_ram64(hart, address, length),
        None => bus.ram64(address, length),
    } {
        bytes.extend_from_slice(bytes_in_ram);
    } else if hart.is_none() && let Ok(address) = u32::try_from(address) {
        bytes = bus.read_bytes(address, length).map_err(|error| error.to_string())?
            .collect::<Result<_, _>>().map_err(|error| error.to_string())?;
    } else if let Some(hart) = hart {
        for offset in 0..length {
            let address = address.checked_add(offset as u64).ok_or("disassembly address overflow")?;
            bytes.push(bus.cpu_ram64(hart, address, 1).map_or(0, |byte| byte[0]));
        }
    } else {
        return Err(format!("disassembly address out of range: {address:#x}"));
    }
    for byte in &bytes {
        write!(input, "0x{byte:02x} ").unwrap();
    }
    input.push('\n');
    let result = crate::llvm_mc_with_args(triple, features, &["--disassemble", "--show-encoding"], &input)?;
    if !result.status.success() {
        return Err(String::from_utf8_lossy(&result.stderr).trim().to_owned());
    }
    let mut text = String::new();
    let mut offset = 0;
    for line in String::from_utf8_lossy(&result.stdout).lines() {
        let Some((mnemonic, encoded)) = line.split_once("# encoding: [").or_else(|| line.split_once("@ encoding: [")) else {
            continue;
        };
        let Some(opcode) = encoded.strip_suffix(']').and_then(|encoded| encoded.split(',')
            .map(|byte| u8::from_str_radix(byte.trim().strip_prefix("0x")?, 16).ok())
            .collect::<Option<Vec<_>>>()) else {
            break;
        };
        if !bytes[offset..].starts_with(&opcode) {
            break;
        }
        write!(text, "0x{:08x}:   ", address.wrapping_add(offset as u64)).unwrap();
        let group = if triple.starts_with("thumb") { 2 } else { opcode.len() };
        for chunk in opcode.chunks(group) {
            for byte in chunk.iter().rev() {
                write!(text, "{byte:02x}").unwrap();
            }
        }
        let mnemonic = mnemonic.trim();
        let separator = if mnemonic.contains(char::is_whitespace) { "" } else { "\t" };
        writeln!(text, "\t{mnemonic}{separator}").unwrap();
        offset += opcode.len();
    }
    if offset < bytes.len() {
        writeln!(text, "Disassembly error detected. The rest of the output was truncated.").unwrap();
    }
    Ok(text)
}

fn inline_encoding(text: &str) -> Result<Option<Vec<u8>>, String> {
    let mut image = Vec::new();
    for line in text.lines() {
        let Some((_, encoding)) = line.split_once("# encoding: [")
            .or_else(|| line.split_once("@ encoding: ["))
        else {
            continue;
        };
        let encoding = encoding
            .strip_suffix(']')
            .ok_or("invalid llvm-mc instruction encoding")?;
        let bytes = encoding
            .split(',')
            .map(|byte| {
                byte.trim()
                    .strip_prefix("0x")
                    .and_then(|hex| u8::from_str_radix(hex, 16).ok())
            })
            .collect::<Option<Vec<_>>>();
        let Some(bytes) = bytes else {
            return Ok(None);
        };
        if !matches!(bytes.len(), 2 | 4) {
            return Err("unsupported non-standard instruction length".into());
        }
        image.extend(bytes);
    }
    Ok(Some(image))
}

fn link_instructions(
    triple: &str,
    features: &str,
    address: u64,
    source: &str,
) -> Result<Vec<u8>, String> {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    struct Scratch(PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    let dir = std::env::temp_dir().join(format!(
        "penv-monitor-asm-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&dir).map_err(|e| format!("assembly scratch directory: {e}"))?;
    let dir = Scratch(dir);
    let attributes = match triple {
        "thumbv6m-none-eabi" => Some((11, 77, ".thumb")),
        "thumbv7em-none-eabi" => Some((13, 77, ".thumb")),
        "thumbv8m.main-none-eabi" => Some((17, 77, ".thumb")),
        "thumbv8.1m.main-none-eabi" => Some((21, 77, ".thumb")),
        "armv7a-none-eabi" => Some((10, 65, ".arm")),
        "armv7r-none-eabi" => Some((10, 82, ".arm")),
        "armv8r-none-eabi" => Some((15, 82, ".arm")),
        _ => None,
    }.map(|(arch, profile, mode)| format!(".eabi_attribute 6, {arch}\n.eabi_attribute 7, {profile}\n{mode}\n{source}"));
    let source = attributes.as_deref().unwrap_or(source);
    let object = crate::llvm_mc_for(triple, features, "--filetype=obj", source)?;
    if !object.status.success() {
        return Err(format!(
            "llvm-mc {triple}: {}",
            String::from_utf8_lossy(&object.stderr).trim()
        ));
    }
    let input = dir.0.join("instructions.o");
    let output = dir.0.join("instructions.bin");
    fs::write(&input, object.stdout).map_err(|e| e.to_string())?;
    let linked = Command::new("ld.lld")
        .args([
            "--oformat=binary",
            "--no-relax",
            "--entry=0",
            "-Ttext",
            &format!("{address:#x}"),
            "-o",
        ])
        .arg(&output)
        .arg(&input)
        .output()
        .map_err(|e| format!("could not start ld.lld: {e}"))?;
    if !linked.status.success() || !linked.stderr.is_empty() {
        return Err(format!(
            "ld.lld {triple}: {}",
            String::from_utf8_lossy(&linked.stderr).trim()
        ));
    }
    fs::read(output).map_err(|e| e.to_string())
}

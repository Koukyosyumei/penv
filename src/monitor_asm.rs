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
    let instructions: Vec<_> = source
        .lines()
        .map(|line| line.split_once('#').map_or(line, |(code, _)| code))
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
                    || instruction.trim_start().starts_with('.')
                    || instruction.contains(':')
            } else {
                line.starts_with('.')
            }
        })
    {
        return Err("monitor assembly requires instructions and optional local labels".into());
    }
    let (hart, triple, features) = machine.monitor_assembly_target(cpu)?;
    if *requested_triple != "default" && *requested_triple != triple {
        return Err(format!(
            "assembler triple {requested_triple} is unavailable for {triple}"
        ));
    }
    let instructions = instructions
        .into_iter()
        .map(normalize_vector_memory_operand)
        .collect::<Vec<_>>()
        .join(
            "
",
        )
        + "
";
    let output = crate::llvm_mc_for(triple, &features, "--show-encoding", &instructions)?;
    if !output.status.success() || !output.stderr.is_empty() {
        return Err(format!(
            "llvm-mc {triple}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let output_text = String::from_utf8_lossy(&output.stdout);
    let image = match inline_encoding(&output_text)? {
        Some(image) if !output_text.contains("fixup ") => image,
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

fn inline_encoding(text: &str) -> Result<Option<Vec<u8>>, String> {
    let mut image = Vec::new();
    for line in text.lines() {
        let Some((_, encoding)) = line.split_once("# encoding: [") else {
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
    let object = crate::llvm_mc_for(triple, features, "--filetype=obj", source)?;
    if !object.status.success() || !object.stderr.is_empty() {
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

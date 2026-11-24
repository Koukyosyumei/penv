//! RISC-V monitor assembly using the same LLVM assembler as the CLI.
use crate::machine::Machine;
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

pub(crate) fn handle(machine: &mut Machine, command: &str) -> Option<Result<(), String>> {
    let command = command.strip_prefix("AssembleMonitorBlock ")?;
    Some(assemble(machine, command))
}

fn assemble(machine: &mut Machine, command: &str) -> Result<(), String> {
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
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    if instructions.is_empty()
        || instructions.iter().any(|line| {
            if let Some((label, instruction)) = line.split_once(':') {
                label.is_empty()
                    || !label
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
                    || instruction.trim_start().starts_with('.')
                    || instruction.contains([';', ':'])
            } else {
                line.starts_with('.') || line.contains(';')
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
    let instructions = instructions.join(
        "
",
    ) + "
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
    Ok(())
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

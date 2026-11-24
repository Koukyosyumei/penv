//! RISC-V monitor assembly using the same LLVM assembler as the CLI.
use crate::machine::Machine;

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
        || instructions
            .iter()
            .any(|line| line.starts_with('.') || line.contains([';', ':']))
    {
        return Err(
            "monitor assembly supports instruction blocks without directives or labels".into(),
        );
    }
    let (hart, triple, features) = machine.monitor_assembly_target(cpu)?;
    if *requested_triple != "default" && *requested_triple != triple {
        return Err(format!("assembler triple {requested_triple} is unavailable for {triple}"));
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
    let mut image = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
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
            .collect::<Option<Vec<_>>>()
            .ok_or("unsupported unresolved assembly relocation")?;
        if !matches!(bytes.len(), 2 | 4) {
            return Err("unsupported non-standard instruction length".into());
        }
        image.extend(bytes);
    }
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

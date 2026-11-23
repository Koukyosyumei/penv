//! Cortex-M4 assembly and disassembly using the host LLVM MC tool.
use std::{
    ffi::OsString,
    io::Write,
    process::{Command, Stdio},
};

fn llvm_mc(option: &str, input: &str) -> Result<String, String> {
    let mut child = Command::new("llvm-mc")
        .args(["-triple=thumbv7em-none-eabi", "-mcpu=cortex-m4", option])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("starting llvm-mc: {error}"))?;
    let input_result = child.stdin.take().unwrap().write_all(input.as_bytes());
    let result = child
        .wait_with_output()
        .map_err(|error| error.to_string())?;
    if !result.status.success() || !result.stderr.is_empty() {
        return Err(String::from_utf8_lossy(&result.stderr).trim().to_owned());
    }
    input_result.map_err(|error| error.to_string())?;
    String::from_utf8(result.stdout).map_err(|error| error.to_string())
}

fn assemble(source: &str) -> Result<String, String> {
    let result = llvm_mc(
        "--show-encoding",
        &format!(".syntax unified\n.thumb\n{source}\n"),
    )?;
    let mut bytes = String::new();
    for line in result.lines().map(str::trim) {
        if line.is_empty() || matches!(line, ".text" | ".code\t16") {
            continue;
        }
        let (_, encoding) = line
            .split_once("@ encoding: [")
            .ok_or("expected an instruction with resolved Thumb encoding")?;
        for byte in encoding
            .strip_suffix(']')
            .ok_or("invalid LLVM encoding")?
            .split(',')
        {
            let value = byte
                .trim()
                .strip_prefix("0x")
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
                .ok_or("unresolved Thumb encoding")?;
            bytes.push_str(&format!("{value:02x}"));
        }
    }
    if bytes.is_empty() {
        return Err("expected at least one Thumb instruction".into());
    }
    Ok(bytes)
}

fn disassemble(address: u32, hex: &str) -> Result<String, String> {
    if address & 1 != 0
        || !matches!(hex.len(), 4 | 8)
        || !hex.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("expected a halfword-aligned address and 2 or 4 instruction bytes".into());
    }
    let bytes: Vec<_> = hex
        .as_bytes()
        .chunks_exact(2)
        .map(|part| u8::from_str_radix(std::str::from_utf8(part).unwrap(), 16).unwrap())
        .collect();
    let halfword = u16::from_le_bytes([bytes[0], bytes[1]]);
    if (halfword >= 0xe800) != (bytes.len() == 4) {
        return Err("disassembly error detected: Thumb instruction width mismatch".into());
    }
    let input = bytes
        .iter()
        .map(|byte| format!("0x{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ");
    let result = llvm_mc("--disassemble", &format!("{input}\n"))
        .map_err(|error| format!("disassembly error detected: {error}"))?;
    let mut instructions = result
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && *line != ".text");
    let instruction = instructions
        .next()
        .filter(|_| instructions.next().is_none())
        .ok_or("disassembly error detected: expected one instruction")?;
    let encoding = if bytes.len() == 4 {
        format!(
            "{halfword:04x}{:04x}",
            u16::from_le_bytes([bytes[2], bytes[3]])
        )
    } else {
        format!("{halfword:04x}")
    };
    Ok(format!(
        "{address:#010x}: {encoding} {}",
        instruction.replace('\t', " ")
    ))
}

pub fn run(command: &str, args: impl Iterator<Item = OsString>) -> Result<(), String> {
    let args = args
        .map(|arg| {
            arg.into_string()
                .map_err(|_| "expected UTF-8 arguments".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let result = match (command, args.as_slice()) {
        ("thumb-assemble", [source]) => assemble(source),
        ("thumb-disassemble", [address, bytes]) => {
            disassemble(crate::parse_address(address)?, bytes)
        }
        _ => Err("see --help for Thumb assembly and disassembly arguments".into()),
    }?;
    println!("{result}");
    Ok(())
}

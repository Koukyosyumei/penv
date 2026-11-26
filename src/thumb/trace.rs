use std::{
    collections::HashMap,
    fs::File,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
};

pub struct TraceConfig {
    pub path: PathBuf,
    pub binary: bool,
}

impl TraceConfig {
    pub fn new(path: PathBuf, format: &str, binary: bool) -> Result<Self, String> {
        Format::parse(format, binary)?;
        Ok(Self { path, binary })
    }
}

#[derive(Clone, Copy)]
enum Format {
    Pc,
    Opcode,
    PcAndOpcode,
    Disassembly,
}

impl Format {
    fn parse(format: &str, binary: bool) -> Result<Self, String> {
        match format {
            "PC" => Ok(Self::Pc),
            "Opcode" => Ok(Self::Opcode),
            "PCAndOpcode" => Ok(Self::PcAndOpcode),
            "Disassembly" if binary => {
                Err("don't support binary output file with the Disassembly formatting".into())
            }
            "Disassembly" => Ok(Self::Disassembly),
            _ => Err(format!("unsupported execution tracing format {format:?}")),
        }
    }
}

pub struct Trace {
    output: BufWriter<File>,
    format: Format,
    binary: bool,
    pc_size: u8,
    triple: &'static str,
    features: String,
    disassembly: HashMap<(u64, usize), String>,
}

impl Trace {
    pub fn create(
        path: &Path,
        format: &str,
        binary: bool,
        target: (&'static str, String, &str),
    ) -> Result<Self, String> {
        let format = Format::parse(format, binary)?;
        let (triple, features, model) = target;
        let pc_size = if matches!(format, Format::Opcode) {
            0
        } else if matches!(triple, "riscv64" | "aarch64") {
            8
        } else {
            4
        };
        let mut output = BufWriter::new(
            File::create(path).map_err(|error| format!("{}: {error}", path.display()))?,
        );
        let mut header = || -> std::io::Result<()> {
            output.write_all(b"ReTrace\x05")?;
            let include_opcode = !matches!(format, Format::Pc);
            output.write_all(&[pc_size, u8::from(include_opcode)])?;
            if include_opcode {
                let label = if triple.starts_with("thumb") {
                    "thumb"
                } else {
                    triple
                };
                let target_name = format!("{label} {model}");
                let size = u8::try_from(target_name.len()).map_err(std::io::Error::other)?;
                output.write_all(&[1, size])?;
                output.write_all(target_name.as_bytes())?;
            }
            Ok(())
        };
        if binary {
            header().map_err(|error| format!("writing trace header: {error}"))?;
        }
        Ok(Self {
            output,
            format,
            binary,
            pc_size,
            triple,
            features,
            disassembly: HashMap::new(),
        })
    }

    pub fn record(&mut self, pc: u64, opcode: u64, width: usize) -> Result<(), String> {
        let mut bytes = opcode.to_le_bytes();
        if self.triple.starts_with("thumb") && width == 4 {
            bytes[..4].rotate_left(2);
        }
        let bytes = &bytes[..width];
        if self.binary {
            self.output
                .write_all(&pc.to_le_bytes()[..self.pc_size as usize])
                .and_then(|()| {
                    if !matches!(self.format, Format::Pc) {
                        self.output.write_all(&[width as u8])?;
                        self.output.write_all(bytes)?;
                    }
                    self.output.write_all(&[0])
                })
                .map_err(|error| format!("writing trace: {error}"))?;
            return Ok(());
        }
        let digits = width * 2;
        let result = match self.format {
            Format::Pc => writeln!(self.output, "0x{pc:X}"),
            Format::Opcode => writeln!(self.output, "0x{opcode:0digits$X}"),
            Format::PcAndOpcode => writeln!(self.output, "0x{pc:X}: 0x{opcode:0digits$X}"),
            Format::Disassembly => {
                // Bound the cache to keep long-running traces at fixed memory use.
                if self.disassembly.len() >= 4096 {
                    self.disassembly.clear();
                }
                let key = (opcode, width);
                if let std::collections::hash_map::Entry::Vacant(entry) =
                    self.disassembly.entry(key)
                {
                    let input = bytes
                        .iter()
                        .map(|byte| format!("0x{byte:02x}"))
                        .collect::<Vec<_>>()
                        .join(" ");
                    let result = crate::llvm_mc_with_args(
                        self.triple,
                        &self.features,
                        &["--disassemble", "--show-encoding"],
                        &format!("{input}\n"),
                    )?;
                    let decoder = String::from_utf8_lossy(&result.stdout);
                    let text = decoder
                        .lines()
                        .find_map(|line| {
                            line.split_once("# encoding:")
                                .or_else(|| line.split_once("@ encoding:"))
                                .map(|(text, _)| text.trim().to_owned())
                        })
                        .unwrap_or_else(|| format!(".word 0x{opcode:0digits$X}"));
                    entry.insert(text);
                }
                writeln!(
                    self.output,
                    "0x{pc:X}: 0x{opcode:0digits$X} {}",
                    self.disassembly[&key]
                )
            }
        };
        result.map_err(|error| format!("writing trace: {error}"))
    }

    pub fn finish(mut self) -> Result<(), String> {
        self.output
            .flush()
            .map_err(|error| format!("flushing trace: {error}"))
    }
}

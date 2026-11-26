use std::{
    collections::HashMap,
    fs::File,
    io::{BufWriter, Seek, SeekFrom, Write},
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

#[derive(Clone, Copy)]
pub(crate) enum Tracking {
    Memory,
    Vector,
    Atomics,
}

pub struct Trace {
    pub(crate) name: String,
    pending: bool,
    memory: bool,
    vector: bool,
    atomics: bool,
    output: BufWriter<File>,
    format: Format,
    binary: bool,
    pc_size: u8,
    triple: &'static str,
    features: String,
    disassembly: HashMap<(u64, usize, bool), String>,
    block: Option<TraceBlock>,
}

struct TraceBlock {
    thumb: bool,
    count_at: u64,
    count: u64,
    next: u64,
}

impl Trace {
    pub fn create(
        name: &str,
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
                let label = triple.split('-').next().unwrap();
                let mixed = label.starts_with("arm") && label != "arm64";
                if mixed {
                    output.write_all(&[2])?;
                    for target in [label, "thumb"] {
                        let target_name = format!("{target} {model}");
                        let size =
                            u8::try_from(target_name.len()).map_err(std::io::Error::other)?;
                        output.write_all(&[size])?;
                        output.write_all(target_name.as_bytes())?;
                    }
                } else {
                    let label = if label.starts_with("thumb") {
                        "thumb"
                    } else {
                        label
                    };
                    let target_name = format!("{label} {model}");
                    let size = u8::try_from(target_name.len()).map_err(std::io::Error::other)?;
                    output.write_all(&[1, size])?;
                    output.write_all(target_name.as_bytes())?;
                }
            }
            Ok(())
        };
        if binary {
            header().map_err(|error| format!("writing trace header: {error}"))?;
        }
        Ok(Self {
            name: name.into(),
            pending: false,
            memory: false,
            vector: false,
            atomics: false,
            output,
            format,
            binary,
            pc_size,
            triple,
            features,
            disassembly: HashMap::new(),
            block: None,
        })
    }

    pub(crate) fn track(&mut self, tracking: Tracking) {
        match tracking {
            Tracking::Memory => self.memory = true,
            Tracking::Vector => self.vector = true,
            Tracking::Atomics => self.atomics = true,
        }
    }

    pub(crate) fn tracks_atomics(&self) -> bool {
        self.atomics
    }

    fn close_record(&mut self) -> Result<(), String> {
        if self.binary && self.pending {
            self.output
                .write_all(&[0])
                .map_err(|error| format!("writing trace: {error}"))?;
        }
        self.pending = false;
        Ok(())
    }

    pub(crate) fn memory_access(
        &mut self,
        operation: u8,
        address: u64,
        physical: u64,
        value: u64,
    ) -> Result<(), String> {
        if !self.memory || !self.pending {
            return Ok(());
        }
        let result = if self.binary {
            self.output
                .write_all(&[1, operation])
                .and_then(|()| self.output.write_all(&address.to_le_bytes()))
                .and_then(|()| self.output.write_all(&value.to_le_bytes()))
                .and_then(|()| self.output.write_all(&physical.to_le_bytes()))
        } else {
            let kind =
                ["MemoryIORead", "MemoryIOWrite", "MemoryRead", "MemoryWrite"][operation as usize];
            if address == physical {
                writeln!(
                    self.output,
                    "{kind} with address 0x{address:X}, value 0x{value:X}"
                )
            } else {
                writeln!(
                    self.output,
                    "{kind} with address 0x{address:X} => 0x{physical:X}, value 0x{value:X}"
                )
            }
        };
        result.map_err(|error| format!("writing memory trace: {error}"))
    }

    pub(crate) fn vector_configuration(&mut self, length: u64, kind: u64) -> Result<(), String> {
        if !self.vector || !self.pending {
            return Ok(());
        }
        let result = if self.binary {
            self.output
                .write_all(&[2])
                .and_then(|()| self.output.write_all(&length.to_le_bytes()))
                .and_then(|()| self.output.write_all(&kind.to_le_bytes()))
        } else {
            writeln!(
                self.output,
                "Vector configured to VL: 0x{length:X}, VTYPE: 0x{kind:X}"
            )
        };
        result.map_err(|error| format!("writing vector configuration trace: {error}"))
    }

    pub(crate) fn atomic_operands(
        &mut self,
        record: &crate::rv64::AtomicTrace,
    ) -> Result<(), String> {
        if !self.atomics || !self.pending {
            return Ok(());
        }
        let result = (|| -> std::io::Result<()> {
            for (phase, values) in [record.before, record.after].into_iter().enumerate() {
                if self.binary {
                    self.output.write_all(&[
                        3,
                        phase as u8,
                        if record.width == 8 { 3 } else { 2 },
                        (record.instruction >> 27) as u8,
                    ])?;
                    for value in values {
                        self.output
                            .write_all(&value.to_le_bytes()[..record.width])?;
                    }
                } else {
                    let [rd, rs1, rs2, value] = values;
                    let phase = if phase == 0 { "before" } else { "after" };
                    writeln!(
                        self.output,
                        "AMO operands {phase} - RD: 0x{rd:X}, RS1: 0x{rs1:X} (memory value: 0x{value:X}), RS2: 0x{rs2:X}"
                    )?;
                }
            }
            Ok(())
        })();
        result.map_err(|error| format!("writing atomic operands trace: {error}"))
    }

    fn close_block(&mut self) -> std::io::Result<()> {
        if let Some(block) = self.block.take() {
            let end = self.output.stream_position()?;
            self.output.seek(SeekFrom::Start(block.count_at))?;
            self.output.write_all(&block.count.to_le_bytes())?;
            self.output.seek(SeekFrom::Start(end))?;
        }
        Ok(())
    }

    fn extend_block(&mut self, pc: u64, width: usize, thumb: bool) -> std::io::Result<()> {
        if let Some(block) = self.block.as_mut()
            && block.thumb == thumb
            && block.next == pc
        {
            block.count += 1;
            block.next = pc.wrapping_add(width as u64);
            return Ok(());
        }
        self.close_block()?;
        self.output.write_all(&[u8::from(thumb)])?;
        let count_at = self.output.stream_position()?;
        self.output.write_all(&[0; 8])?;
        self.block = Some(TraceBlock {
            thumb,
            count_at,
            count: 1,
            next: pc.wrapping_add(width as u64),
        });
        Ok(())
    }

    pub fn record(
        &mut self,
        pc: u64,
        opcode: u64,
        width: usize,
        thumb: bool,
    ) -> Result<(), String> {
        self.close_record()?;
        if self.binary && !matches!(self.format, Format::Pc) && self.triple.starts_with("arm") {
            self.extend_block(pc, width, thumb)
                .map_err(|error| format!("writing trace block: {error}"))?;
        }
        self.pending = true;
        let mut bytes = opcode.to_le_bytes();
        if thumb && width == 4 {
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
                    Ok(())
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
                let key = (opcode, width, thumb);
                if let std::collections::hash_map::Entry::Vacant(entry) =
                    self.disassembly.entry(key)
                {
                    let input = bytes
                        .iter()
                        .map(|byte| format!("0x{byte:02x}"))
                        .collect::<Vec<_>>()
                        .join(" ");
                    let triple = if thumb {
                        match self.triple {
                            "armv7a-none-eabi" => "thumbv7a-none-eabi",
                            "armv7r-none-eabi" => "thumbv7r-none-eabi",
                            "armv8r-none-eabi" => "thumbv8r-none-eabi",
                            triple => triple,
                        }
                    } else {
                        self.triple
                    };
                    let result = crate::llvm_mc_with_args(
                        triple,
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
                                .map(|(text, _)| {
                                    text.split_whitespace().collect::<Vec<_>>().join(" ")
                                })
                        })
                        .unwrap_or_else(|| format!(".word 0x{opcode:0digits$X}"));
                    entry.insert(text);
                }
                writeln!(
                    self.output,
                    "0x{pc:08x}: {opcode:0digits$x} {}",
                    self.disassembly[&key]
                )
            }
        };
        result.map_err(|error| format!("writing trace: {error}"))
    }

    pub(crate) fn flush(&mut self) -> Result<(), String> {
        self.output
            .flush()
            .map_err(|error| format!("flushing trace: {error}"))
    }

    pub fn finish(mut self) -> Result<(), String> {
        self.close_record()?;
        self.close_block()
            .map_err(|error| format!("closing trace block: {error}"))?;
        self.output
            .flush()
            .map_err(|error| format!("flushing trace: {error}"))
    }
}

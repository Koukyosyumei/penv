use std::{
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
        if binary && format == "Disassembly" {
            return Err("don't support binary output file with the Disassembly formatting".into());
        }
        if format != "PCAndOpcode" {
            return Err(format!("unsupported execution tracing format {format:?}"));
        }
        Ok(Self { path, binary })
    }
}

pub struct Trace {
    output: BufWriter<File>,
    binary: bool,
}

impl Trace {
    pub fn create(path: &Path, binary: bool) -> Result<Self, String> {
        let mut output = BufWriter::new(
            File::create(path).map_err(|error| format!("{}: {error}", path.display()))?,
        );
        if binary {
            output
                .write_all(b"ReTrace\x05\x04\x01\x01\x0fthumb cortex-m4")
                .map_err(|error| format!("writing trace header: {error}"))?;
        }
        Ok(Self { output, binary })
    }

    pub fn record(&mut self, address: u32, opcode: u32, wide: bool) -> Result<(), String> {
        self.write_record(address, opcode, wide)
            .map_err(|error| format!("writing trace: {error}"))
    }

    fn write_record(&mut self, address: u32, opcode: u32, wide: bool) -> std::io::Result<()> {
        if self.binary {
            self.output.write_all(&address.to_le_bytes())?;
            self.output.write_all(&[if wide { 4 } else { 2 }])?;
            if wide {
                self.output
                    .write_all(&((opcode >> 16) as u16).to_le_bytes())?;
            }
            self.output.write_all(&(opcode as u16).to_le_bytes())?;
            self.output.write_all(&[0])?; // no auxiliary data
        } else {
            writeln!(self.output, "0x{address:X}: 0x{opcode:X}")?;
        }
        Ok(())
    }

    pub fn finish(mut self) -> Result<(), String> {
        self.output
            .flush()
            .map_err(|error| format!("flushing trace: {error}"))
    }
}

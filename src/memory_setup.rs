use crate::{bus::Bus, parse_address64};

pub enum MemorySetup {
    Read {
        address: u64,
        width: usize,
    },
    Write {
        address: u64,
        width: usize,
        value: u64,
    },
}

impl MemorySetup {
    pub fn parse(option: &str, argument: &str) -> Result<Self, String> {
        let mut parts = argument.split(':');
        let address = parse_address64(parts.next().unwrap_or_default())?;
        let width = parts
            .next()
            .and_then(|width| width.parse::<usize>().ok())
            .filter(|width| matches!(width, 1 | 2 | 4 | 8))
            .ok_or("memory access width must be 1, 2, 4 or 8 bytes")?;
        match (option, parts.next(), parts.next()) {
            ("--read-memory-now", None, None) => Ok(Self::Read { address, width }),
            ("--write-memory", Some(value), None) => Ok(Self::Write {
                address,
                width,
                value: write_value(value, width)?,
            }),
            _ => Err(
                "expected --read-memory-now ADDRESS:WIDTH or --write-memory ADDRESS:WIDTH:VALUE"
                    .into(),
            ),
        }
    }

    pub fn apply(&self, bus: &mut Bus) -> Result<(), String> {
        match *self {
            Self::Read { address, width } => {
                let source = bus.ram64(address, width).ok_or_else(|| {
                    format!("unmapped RAM read at {address:#018x} ({width} bytes)")
                })?;
                let mut bytes = [0; 8];
                bytes[..width].copy_from_slice(source);
                let value = u64::from_le_bytes(bytes);
                eprintln!("penv: memory sysbus@{address:#018x}:{width} = {value:#018x}");
            }
            Self::Write {
                address,
                width,
                value,
            } => {
                let target = bus.memory64(address, width).ok_or_else(|| {
                    format!("unmapped RAM write at {address:#018x} ({width} bytes)")
                })?;
                target.copy_from_slice(&value.to_le_bytes()[..width]);
            }
        }
        Ok(())
    }
}

fn write_value(text: &str, width: usize) -> Result<u64, String> {
    let bits = width * 8;
    let mask = u64::MAX >> (64 - bits);
    let too_large = || "--write-memory value exceeds the requested width".to_owned();
    if let Some(magnitude) = text.strip_prefix('-') {
        let magnitude = parse_address64(magnitude)?;
        if magnitude > 1u64 << (bits - 1) {
            return Err(too_large());
        }
        Ok(magnitude.wrapping_neg() & mask)
    } else {
        let value = parse_address64(text)?;
        if value > mask {
            return Err(too_large());
        }
        Ok(value)
    }
}

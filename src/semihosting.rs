use std::{
    collections::{BTreeMap, VecDeque},
    io::Write,
};

use crate::bus::Bus;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Exit {
    pub reason: u32,
    pub code: Option<i32>,
}

impl std::fmt::Display for Exit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let reason = match self.reason {
            0x20023 => "ADP_Stopped_RunTimeErrorUnknown",
            0x20026 => "ADP_Stopped_ApplicationExit",
            _ => "",
        };
        write!(
            f,
            "Program exited with reason {reason}(0x{:X})",
            self.reason
        )?;
        if let Some(code) = self.code {
            write!(f, " and return code {code}")?;
        }
        Ok(())
    }
}

const FEATURES: &[u8] = b"SHFB\x03";

enum Descriptor {
    Console(u32),
    Features(usize),
}

#[derive(Default)]
pub struct Semihosting {
    pub console: bool,
    pub split_error: bool,
    pub program_arguments: String,
    descriptors: BTreeMap<u32, Descriptor>,
    input: VecDeque<u8>,
    errno: u32,
}

impl Semihosting {
    pub fn reset(&mut self) {
        self.descriptors.clear();
        self.input.clear();
        self.errno = 0;
    }

    pub fn queue_input(&mut self, bytes: Vec<u8>) {
        self.input.extend(bytes);
    }

    pub fn call(
        &mut self,
        bus: &mut Bus,
        operation: u32,
        argument: u32,
        output: &mut impl Write,
    ) -> Result<(u32, Option<Exit>), String> {
        let field = |offset| {
            let address = argument
                .checked_add(offset)
                .ok_or("semihosting argument address overflow")?;
            let bytes = bus
                .memory_slice(address, 4)
                .map_err(|error| format!("semihosting: {error}"))?;
            Ok::<u32, String>(u32::from_le_bytes(bytes.try_into().unwrap()))
        };
        let write = |output: &mut dyn Write, bytes: &[u8]| {
            output
                .write_all(bytes)
                .map_err(|error| format!("semihosting: {error}"))
        };
        let unsuccessful = u32::MAX;
        let result = match operation {
            0x01 => {
                let address = field(0)?;
                let mode = field(4)?;
                let length = field(8)?;
                let path = length
                    .checked_add(1)
                    .and_then(|size| bus.memory_slice(address, size as usize).ok());
                let descriptor = match path {
                    Some(b":tt\0") if self.console && mode <= 11 => Some(Descriptor::Console(mode)),
                    Some(b":semihosting-features\0") if mode <= 1 => Some(Descriptor::Features(0)),
                    _ => None,
                };
                if let Some(descriptor) = descriptor {
                    let handle = (1..)
                        .find(|handle| !self.descriptors.contains_key(handle))
                        .unwrap();
                    self.descriptors.insert(handle, descriptor);
                    handle
                } else {
                    unsuccessful
                }
            }
            0x02 => {
                if self.descriptors.remove(&field(0)?).is_some() {
                    0
                } else {
                    unsuccessful
                }
            }
            0x03 if self.console => {
                write(output, &[field(0)? as u8])?;
                0
            }
            0x04 if self.console => {
                let mut address = argument;
                loop {
                    let byte = bus
                        .read_ram::<1>(address)
                        .map_err(|error| format!("semihosting: {error}"))?
                        as u8;
                    if byte == 0 {
                        break;
                    }
                    write(output, &[byte])?;
                    address = address
                        .checked_add(1)
                        .ok_or("semihosting string address overflow")?;
                }
                0
            }
            0x05 => {
                let handle = field(0)?;
                let address = field(4)?;
                let length = field(8)?;
                match self.descriptors.get(&handle) {
                    Some(&Descriptor::Console(mode)) if mode >= 2 => {
                        if length != 0 {
                            let bytes = bus
                                .memory_slice(address, length as usize)
                                .map_err(|error| format!("semihosting: {error}"))?;
                            if self.split_error && mode >= 8 {
                                write(&mut std::io::stderr().lock(), bytes)?;
                            } else {
                                write(output, bytes)?;
                            }
                        }
                        0
                    }
                    _ => length,
                }
            }
            0x06 => {
                let handle = field(0)?;
                let address = field(4)?;
                let length = field(8)?;
                let descriptor = self.descriptors.get_mut(&handle);
                let count = match descriptor {
                    Some(Descriptor::Features(offset)) => {
                        let count = (length as usize).min(FEATURES.len() - *offset);
                        if count != 0 {
                            bus.memory(address, count)
                                .map_err(|error| format!("semihosting: {error}"))?
                                .copy_from_slice(&FEATURES[*offset..*offset + count]);
                            *offset += count;
                        }
                        count
                    }
                    Some(Descriptor::Console(0..=3 | 6..=7)) => {
                        let count = (length as usize).min(self.input.len());
                        if count != 0 {
                            let destination = bus
                                .memory(address, count)
                                .map_err(|error| format!("semihosting: {error}"))?;
                            for (byte, received) in
                                destination.iter_mut().zip(self.input.drain(..count))
                            {
                                *byte = received;
                            }
                        }
                        count
                    }
                    _ => {
                        self.errno = 9; // EBADF
                        0
                    }
                };
                length - count as u32
            }
            0x07 if self.console && argument == 0 => u32::from(self.input.pop_front().unwrap_or(0)),
            0x09 => match self.descriptors.get(&field(0)?) {
                Some(Descriptor::Console(_)) => 1,
                Some(Descriptor::Features(_)) => 0,
                None => unsuccessful,
            },
            0x0c => match self.descriptors.get(&field(0)?) {
                Some(Descriptor::Features(_)) => FEATURES.len() as u32,
                _ => unsuccessful,
            },
            0x13 => self.errno,
            0x15 => {
                let address = field(0)?;
                let capacity = field(4)? as usize;
                let mut line = String::from("program-name");
                if !self.program_arguments.is_empty() {
                    line.push(' ');
                    line.push_str(&self.program_arguments);
                }
                line.push('\0');
                if line.len() > capacity {
                    self.errno = 75; // EOVERFLOW
                    unsuccessful
                } else {
                    bus.memory(address, line.len())
                        .map_err(|error| format!("semihosting: {error}"))?
                        .copy_from_slice(line.as_bytes());
                    bus.memory(argument + 4, 4)
                        .map_err(|error| format!("semihosting: {error}"))?
                        .copy_from_slice(&(line.len() as u32).to_le_bytes());
                    0
                }
            }
            0x18 | 0x20 => {
                let (reason, code) = if operation == 0x18 {
                    (argument, None)
                } else {
                    (field(0)?, Some(field(4)? as i32))
                };
                return Ok((operation, Some(Exit { reason, code })));
            }
            _ => unsuccessful,
        };
        Ok((result, None))
    }
}

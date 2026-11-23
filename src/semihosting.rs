use std::{
    collections::{BTreeMap, VecDeque},
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
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
    File(File),
}

#[derive(Default)]
pub struct Semihosting {
    pub console: bool,
    pub split_error: bool,
    pub program_arguments: String,
    pub working_directory: Option<PathBuf>,
    pub temporary_files_directory: Option<String>,
    created_temporary_dir: Option<PathBuf>,
    generated_temporary_name: Option<String>,
    descriptors: BTreeMap<u32, Descriptor>,
    input: VecDeque<u8>,
    errno: u32,
    previous_operation: u32,
}

impl Semihosting {
    pub fn reset(&mut self) {
        self.descriptors.clear();
        self.input.clear();
        self.errno = 0;
        self.previous_operation = 0;
        self.clean_temporary_directory();
    }

    pub fn queue_input(&mut self, bytes: Vec<u8>) {
        self.input.extend(bytes);
    }

    fn file_path(&self, name: &[u8]) -> io::Result<PathBuf> {
        let denied = || io::Error::from(io::ErrorKind::PermissionDenied);
        let dir = self.working_directory.as_ref().ok_or_else(denied)?;
        let name = std::str::from_utf8(name).map_err(|_| denied())?;
        let mut components = Path::new(name).components();
        let Some(Component::Normal(name)) = components.next() else {
            return Err(denied());
        };
        let mut path = dir.join(name);
        match components.next() {
            None => {}
            Some(Component::Normal(file))
                if Some(name) == self.temporary_name().map(std::ffi::OsStr::new)
                    && components.next().is_none()
                    && fs::symlink_metadata(&path)?.is_dir() =>
            {
                path.push(file);
            }
            _ => return Err(denied()),
        }
        match fs::symlink_metadata(&path) {
            Ok(metadata) if !metadata.is_file() => Err(denied()),
            Ok(_) => Ok(path),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(path),
            Err(error) => Err(error),
        }
    }

    fn open_file(&self, name: &[u8], mode: u32) -> io::Result<File> {
        let mut options = OpenOptions::new();
        match mode {
            0..=1 => options.read(true),
            2..=3 => options.read(true).write(true),
            4..=7 => options
                .write(true)
                .read(mode >= 6)
                .create(true)
                .truncate(true),
            8..=9 => options.append(true).create(true),
            _ => return Err(io::Error::from(io::ErrorKind::InvalidInput)),
        };
        options.open(self.file_path(name)?)
    }

    fn temporary_name(&self) -> Option<&str> {
        self.temporary_files_directory
            .as_deref()
            .or(self.generated_temporary_name.as_deref())
    }

    fn ensure_temporary_directory(&mut self) -> io::Result<&str> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = self
            .working_directory
            .as_ref()
            .ok_or_else(|| io::Error::from(io::ErrorKind::PermissionDenied))?;
        if let Some(name) = self.temporary_name() {
            let path = root.join(name);
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.is_dir() => {}
                Ok(_) => return Err(io::Error::from(io::ErrorKind::PermissionDenied)),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    fs::create_dir(&path)?;
                    eprintln!(
                        "cpu.semihosting: SYS_TMPNAM: Created temporary files directory at: {}",
                        path.display()
                    );
                    self.created_temporary_dir = Some(path);
                }
                Err(error) => return Err(error),
            }
        } else {
            loop {
                let name = format!(
                    "temp-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                );
                let path = root.join(&name);
                match fs::create_dir(&path) {
                    Ok(()) => {
                        eprintln!(
                            "cpu.semihosting: SYS_TMPNAM: Created temporary files directory at: {}",
                            path.display()
                        );
                        self.created_temporary_dir = Some(path);
                        self.generated_temporary_name = Some(name);
                        break;
                    }
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => return Err(error),
                }
            }
        }
        Ok(self.temporary_name().unwrap())
    }

    fn clean_temporary_directory(&mut self) {
        if let Some(path) = self.created_temporary_dir.take() {
            let _ = fs::remove_dir(path);
        }
        self.generated_temporary_name = None;
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
        let previous_operation = std::mem::replace(&mut self.previous_operation, operation);
        let guest_path = |address, length: u32| {
            let bytes = bus
                .memory_slice(address, length.checked_add(1)? as usize)
                .ok()?;
            bytes
                .strip_suffix(&[0])
                .filter(|name| !name.is_empty() && !name.contains(&0))
        };
        let result = match operation {
            0x01 => {
                let address = field(0)?;
                let mode = field(4)?;
                let length = field(8)?;
                let descriptor = match guest_path(address, length) {
                    Some(b":tt") if self.console && mode <= 11 => Some(Descriptor::Console(mode)),
                    Some(b":semihosting-features") if mode <= 1 => Some(Descriptor::Features(0)),
                    Some(name) => match self.open_file(name, mode) {
                        Ok(file) => Some(Descriptor::File(file)),
                        Err(_) => {
                            self.errno = 22; // Upstream maps descriptor creation failures to EINVAL.
                            None
                        }
                    },
                    None => None,
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
                    self.errno = 9;
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
                match self.descriptors.get_mut(&handle) {
                    Some(Descriptor::Console(mode)) if *mode >= 2 => {
                        if length != 0 {
                            let bytes = bus
                                .memory_slice(address, length as usize)
                                .map_err(|error| format!("semihosting: {error}"))?;
                            if self.split_error && *mode >= 8 {
                                write(&mut std::io::stderr().lock(), bytes)?;
                            } else {
                                write(output, bytes)?;
                            }
                        }
                        0
                    }
                    Some(Descriptor::File(file)) => {
                        let bytes = bus
                            .memory_slice(address, length as usize)
                            .map_err(|error| format!("semihosting: {error}"))?;
                        match file.write(bytes) {
                            Ok(count) => length - count as u32,
                            Err(_) => {
                                self.errno = 9;
                                length
                            }
                        }
                    }
                    _ => {
                        self.errno = 9;
                        length
                    }
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
                    Some(Descriptor::File(file)) if length != 0 => {
                        let bytes = bus
                            .memory(address, length as usize)
                            .map_err(|error| format!("semihosting: {error}"))?;
                        match file.read(bytes) {
                            Ok(count) => count,
                            Err(_) => {
                                self.errno = 9;
                                0
                            }
                        }
                    }
                    Some(Descriptor::File(_)) => 0,
                    _ => {
                        self.errno = 9; // EBADF
                        0
                    }
                };
                length - count as u32
            }
            0x07 if self.console && argument == 0 => u32::from(self.input.pop_front().unwrap_or(0)),
            0x08 => {
                let code = field(0)? as i32;
                let error = match previous_operation {
                    0x08 | 0x13 | 0x18 | 0x20 => false,
                    0x09 => !matches!(code, 0 | 1),
                    0x05 | 0x0e | 0x0f => code != 0,
                    _ => code < 0,
                };
                if error { unsuccessful } else { 0 }
            }
            0x09 => match self.descriptors.get(&field(0)?) {
                Some(Descriptor::Console(_)) => 1,
                Some(Descriptor::Features(_) | Descriptor::File(_)) => 0,
                None => {
                    self.errno = 9;
                    unsuccessful
                }
            },
            0x0a => {
                let handle = field(0)?;
                let offset = field(4)?;
                match self.descriptors.get_mut(&handle) {
                    Some(Descriptor::File(file)) => {
                        if file.seek(SeekFrom::Start(u64::from(offset))).is_ok() {
                            0
                        } else {
                            self.errno = 22;
                            unsuccessful
                        }
                    }
                    Some(Descriptor::Features(position)) if offset as usize <= FEATURES.len() => {
                        *position = offset as usize;
                        0
                    }
                    _ => {
                        self.errno = 22;
                        unsuccessful
                    }
                }
            }
            0x0d => {
                let address = field(0)?;
                let id = field(4)?;
                let capacity = field(8)? as usize;
                if id >= 256 {
                    self.errno = 22;
                    return Ok((unsuccessful, None));
                }
                let path = match self.ensure_temporary_directory() {
                    Ok(directory) => format!("{directory}/{id}"),
                    Err(error) => {
                        self.errno = if error.kind() == io::ErrorKind::PermissionDenied {
                            13
                        } else {
                            22
                        };
                        return Ok((unsuccessful, None));
                    }
                };
                if path.len() > capacity {
                    self.errno = 75;
                    unsuccessful
                } else {
                    bus.memory(address, path.len())
                        .map_err(|error| format!("semihosting: {error}"))?
                        .copy_from_slice(path.as_bytes());
                    0
                }
            }
            0x0c => match self.descriptors.get(&field(0)?) {
                Some(Descriptor::Features(_)) => FEATURES.len() as u32,
                Some(Descriptor::File(file)) => match file.metadata() {
                    Ok(metadata) => u32::try_from(metadata.len()).unwrap_or_else(|_| {
                        self.errno = 75;
                        unsuccessful
                    }),
                    Err(_) => {
                        self.errno = 5;
                        unsuccessful
                    }
                },
                _ => {
                    self.errno = 22;
                    unsuccessful
                }
            },
            0x0e => {
                let address = field(0)?;
                let length = field(4)?;
                let result = guest_path(address, length)
                    .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))
                    .and_then(|name| self.file_path(name))
                    .and_then(fs::remove_file);
                match result {
                    Ok(()) => 0,
                    Err(error) => {
                        self.errno = match error.kind() {
                            io::ErrorKind::NotFound => 2,
                            io::ErrorKind::PermissionDenied => 13,
                            io::ErrorKind::InvalidInput => 22,
                            _ => 5,
                        };
                        unsuccessful
                    }
                }
            }
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

impl Drop for Semihosting {
    fn drop(&mut self) {
        self.descriptors.clear();
        self.clean_temporary_directory();
    }
}

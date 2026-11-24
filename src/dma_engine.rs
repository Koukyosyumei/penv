use std::io::Write;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Width {
    Byte = 1,
    Word = 2,
    DoubleWord = 4,
    QuadWord = 8,
}

#[derive(Clone, Copy)]
pub struct Access {
    pub width: Width,
    pub increment: bool,
    /// Unsigned bus displacement, with negative strides encoded in two's complement.
    pub step: u64,
}

impl Access {
    pub fn new(width: Width) -> Self {
        Self {
            width,
            increment: true,
            step: width as u64,
        }
    }

    fn next(self, address: u64) -> u64 {
        if self.increment {
            address.wrapping_add(self.step)
        } else {
            address
        }
    }
}

pub enum Source<'a> {
    Array(&'a [u8], usize),
    Bus(u64),
}

pub enum Destination<'a> {
    Array(&'a mut [u8], usize),
    Bus(u64),
}

pub struct Request<'a> {
    pub source: Source<'a>,
    pub destination: Destination<'a>,
    pub size: usize,
    pub read: Access,
    pub write: Access,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Response {
    pub read_address: Option<u64>,
    pub write_address: Option<u64>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mapping {
    Unmapped,
    MappedMemory,
    Peripheral,
}

pub trait DmaBus {
    type Error;
    fn mapping(&self, address: u64) -> Mapping;
    fn read_unit(&mut self, address: u64, width: Width) -> Result<u64, Self::Error>;
    fn write_unit(
        &mut self,
        address: u64,
        width: Width,
        value: u64,
        output: &mut dyn Write,
    ) -> Result<(), Self::Error>;
    fn read_block(&mut self, address: u64, bytes: &mut [u8]) -> Result<(), Self::Error>;
    fn write_block(
        &mut self,
        address: u64,
        bytes: &[u8],
        output: &mut dyn Write,
    ) -> Result<(), Self::Error>;
}

#[derive(Debug)]
pub enum Error<E> {
    MisalignedSize,
    ArrayBounds,
    Allocation,
    Bus(E),
}

fn continuous(mapping: Mapping, access: Access) -> bool {
    mapping != Mapping::Peripheral && access.step == access.width as u64
}

pub fn issue_copy<B: DmaBus>(
    bus: &mut B,
    request: Request<'_>,
    output: &mut dyn Write,
) -> Result<Response, Error<B::Error>> {
    let Request {
        source,
        destination,
        size,
        read,
        write,
    } = request;
    let (read_width, write_width) = (read.width as usize, write.width as usize);
    if !size.is_multiple_of(read_width) || !size.is_multiple_of(write_width) {
        return Err(Error::MisalignedSize);
    }
    let mut buffer = Vec::new();
    buffer
        .try_reserve_exact(size)
        .map_err(|_| Error::Allocation)?;
    buffer.resize(size, 0);

    let source_mapping = bus.mapping(match source {
        Source::Bus(address) => address,
        Source::Array(..) => 0,
    });
    let source_continuous = continuous(source_mapping, read);
    let read_address = match source {
        Source::Array(bytes, start) => {
            let end = start.checked_add(size).ok_or(Error::ArrayBounds)?;
            buffer.copy_from_slice(bytes.get(start..end).ok_or(Error::ArrayBounds)?);
            None
        }
        Source::Bus(mut address) => {
            if source_continuous && size != 0 {
                let count = if read.increment { size } else { read_width };
                bus.read_block(address, &mut buffer[..count])
                    .map_err(Error::Bus)?;
                if read.increment {
                    address = address.wrapping_add(size as u64);
                }
            } else if source_mapping != Mapping::Unmapped {
                for chunk in buffer.chunks_exact_mut(read_width) {
                    let unit = bus.read_unit(address, read.width).map_err(Error::Bus)?;
                    chunk.copy_from_slice(&unit.to_le_bytes()[..read_width]);
                    address = read.next(address);
                }
            }
            Some(address)
        }
    };

    let write_address = match destination {
        Destination::Array(bytes, start) => {
            let end = start.checked_add(size).ok_or(Error::ArrayBounds)?;
            bytes
                .get_mut(start..end)
                .ok_or(Error::ArrayBounds)?
                .copy_from_slice(&buffer);
            None
        }
        Destination::Bus(mut address) => {
            let mapping = bus.mapping(address);
            if continuous(mapping, write) && size != 0 {
                if !write.increment {
                    bus.write_block(address, &buffer[size - write_width..], output)
                        .map_err(Error::Bus)?;
                } else {
                    if read.increment || !source_continuous {
                        bus.write_block(address, &buffer, output)
                            .map_err(Error::Bus)?;
                    } else {
                        for offset in (0..size).step_by(write_width) {
                            bus.write_block(
                                address.wrapping_add(offset as u64),
                                &buffer[..write_width],
                                output,
                            )
                            .map_err(Error::Bus)?;
                        }
                    }
                    address = address.wrapping_add(size as u64);
                }
            } else if mapping != Mapping::Unmapped {
                for chunk in buffer.chunks_exact(write_width) {
                    let mut unit = [0; 8];
                    unit[..write_width].copy_from_slice(chunk);
                    bus.write_unit(address, write.width, u64::from_le_bytes(unit), output)
                        .map_err(Error::Bus)?;
                    address = write.next(address);
                }
            }
            Some(address)
        }
    };
    Ok(Response {
        read_address,
        write_address,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::Bus;

    #[test]
    fn strided_widths_return_the_next_bus_addresses() {
        let mut bus = Bus::default();
        bus.map_array_memory(0x1000, 16).unwrap();
        bus.map_array_memory(0x2000, 16).unwrap();
        bus.memory64(0x1000, 8)
            .unwrap()
            .copy_from_slice(&[1, 2, 0, 0, 3, 4, 0, 0]);
        let response = issue_copy(
            &mut bus,
            Request {
                source: Source::Bus(0x1004),
                destination: Destination::Bus(0x2000),
                size: 4,
                read: Access {
                    step: (-4_i64) as u64,
                    ..Access::new(Width::Word)
                },
                write: Access {
                    step: 2,
                    ..Access::new(Width::Byte)
                },
            },
            &mut std::io::sink(),
        )
        .unwrap();
        assert_eq!(bus.ram64(0x2000, 8).unwrap(), &[3, 0, 4, 0, 1, 0, 2, 0]);
        assert_eq!(
            response,
            Response {
                read_address: Some(0x0ffc),
                write_address: Some(0x2008)
            }
        );
    }

    #[test]
    fn fixed_mapped_source_repeats_the_first_destination_unit() {
        let mut bus = Bus::default();
        bus.map_ram(0x1000, 16).unwrap();
        bus.map_ram(0x2000, 16).unwrap();
        bus.memory64(0x1000, 4)
            .unwrap()
            .copy_from_slice(&[1, 2, 3, 4]);
        let response = issue_copy(
            &mut bus,
            Request {
                source: Source::Bus(0x1000),
                destination: Destination::Bus(0x2000),
                size: 4,
                read: Access {
                    increment: false,
                    ..Access::new(Width::Word)
                },
                write: Access::new(Width::Word),
            },
            &mut std::io::sink(),
        )
        .unwrap();
        assert_eq!(bus.ram64(0x2000, 4).unwrap(), &[1, 2, 1, 2]);
        assert_eq!(
            response,
            Response {
                read_address: Some(0x1000),
                write_address: Some(0x2004)
            }
        );
    }

    #[test]
    fn fixed_mapped_destination_retains_the_last_unit() {
        let mut bus = Bus::default();
        bus.map_ram(0x2000, 16).unwrap();
        let response = issue_copy(
            &mut bus,
            Request {
                source: Source::Array(&[1, 2, 3, 4], 0),
                destination: Destination::Bus(0x2000),
                size: 4,
                read: Access::new(Width::Byte),
                write: Access {
                    increment: false,
                    ..Access::new(Width::Word)
                },
            },
            &mut std::io::sink(),
        )
        .unwrap();
        assert_eq!(bus.ram64(0x2000, 4).unwrap(), &[3, 4, 0, 0]);
        assert_eq!(
            response,
            Response {
                read_address: None,
                write_address: Some(0x2000)
            }
        );
    }
}

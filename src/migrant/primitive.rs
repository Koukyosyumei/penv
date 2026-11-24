//! Integer framing for the pinned Migrant Release stream format.
use std::io::{self, BufWriter, Read, Write};

const BLOCK: usize = 4096;
const SECOND: i64 = 10_000_000;
const EPOCH: i64 = 630_822_816_000_000_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Decimal {
    pub magnitude: [u32; 3],
    pub negative: bool,
    pub scale: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Guid(pub [u8; 16]); // Guid.ToByteArray order

fn boundary(position: usize) -> usize {
    if position <= 128 {
        128
    } else if position <= 1024 {
        1024
    } else {
        BLOCK
    }
}

fn padding(position: usize) -> usize {
    let boundary = boundary(position);
    (boundary - position % boundary) % boundary
}

enum Output<W: Write> {
    Direct(W),
    Buffered(BufWriter<W>),
}

pub struct Writer<W: Write> {
    output: Output<W>,
    position: usize,
}

impl<W: Write> Writer<W> {
    pub fn new(stream: W, buffered: bool) -> Self {
        let output = if buffered {
            Output::Buffered(BufWriter::with_capacity(BLOCK, stream))
        } else {
            Output::Direct(stream)
        };
        Self {
            output,
            position: 0,
        }
    }

    pub fn bytes(&mut self, bytes: &[u8]) -> io::Result<()> {
        match &mut self.output {
            Output::Direct(output) => output.write_all(bytes)?,
            Output::Buffered(output) => output.write_all(bytes)?,
        }
        self.position += bytes.len();
        Ok(())
    }

    pub fn unsigned(&mut self, mut value: u64) -> io::Result<()> {
        let mut bytes = [0; 10];
        let mut index = 0;
        while value > 127 {
            bytes[index] = value as u8 | 128;
            value >>= 7;
            index += 1;
        }
        bytes[index] = value as u8;
        self.bytes(&bytes[..=index])
    }

    pub fn signed(&mut self, value: i64) -> io::Result<()> {
        self.unsigned(((value << 1) ^ (value >> 63)) as u64)
    }

    pub fn string(&mut self, value: &str) -> io::Result<()> {
        let length = i32::try_from(value.len()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Migrant string length exceeds Int32",
            )
        })?;
        self.signed(length.into())?;
        self.bytes(value.as_bytes())
    }

    pub fn decimal(&mut self, value: Decimal) -> io::Result<()> {
        if value.scale > 28 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid decimal scale",
            ));
        }
        for word in value.magnitude {
            self.signed(i64::from(word as i32))?;
        }
        let flags = (i32::from(value.scale) << 16) | if value.negative { i32::MIN } else { 0 };
        self.signed(flags.into())
    }

    pub fn guid(&mut self, value: Guid) -> io::Result<()> {
        self.bytes(&value.0)
    }

    pub fn date_time(&mut self, ticks: i64) -> io::Result<()> {
        let offset = ticks.checked_sub(EPOCH).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "DateTime epoch offset overflow",
            )
        })?;
        self.time_span(offset)
    }

    pub fn time_span(&mut self, ticks: i64) -> io::Result<()> {
        if ticks % SECOND != 0 {
            self.bytes(&[255])?;
            return self.signed(ticks);
        }
        let seconds = ticks / SECOND;
        self.bytes(&[(seconds / 3600 % 24) as u8])?;
        self.unsigned(((seconds % 3600) as u16).into())?;
        self.signed(seconds / 86_400)
    }

    pub fn finish(mut self) -> io::Result<()> {
        if let Output::Buffered(output) = &mut self.output {
            output.write_all(&[0; BLOCK][..padding(self.position)])?;
            output.flush()?;
        }
        Ok(())
    }

    pub fn copy_from(&mut self, source: &mut impl Read, mut count: usize) -> io::Result<()> {
        let mut buffer = [0; BLOCK];
        while count != 0 {
            let take = count.min(BLOCK);
            source.read_exact(&mut buffer[..take])?;
            self.bytes(&buffer[..take])?;
            count -= take;
        }
        Ok(())
    }
}

struct Buffer {
    bytes: Box<[u8; BLOCK]>,
    loaded: usize,
    used: usize,
}

pub struct Reader<R> {
    stream: R,
    buffer: Option<Buffer>,
    physical_position: usize,
}

impl<R: Read> Reader<R> {
    pub fn new(stream: R, buffered: bool) -> Self {
        Self {
            stream,
            buffer: buffered.then(|| Buffer {
                bytes: Box::new([0; BLOCK]),
                loaded: 0,
                used: 0,
            }),
            physical_position: 0,
        }
    }

    pub fn bytes(&mut self, mut output: &mut [u8]) -> io::Result<()> {
        let Some(buffer) = &mut self.buffer else {
            return self.stream.read_exact(output);
        };
        while !output.is_empty() {
            if buffer.used == buffer.loaded {
                let align = boundary(self.physical_position);
                let count = align - self.physical_position % align;
                self.stream.read_exact(&mut buffer.bytes[..count])?;
                self.physical_position += count;
                buffer.loaded = count;
                buffer.used = 0;
            }
            let count = output.len().min(buffer.loaded - buffer.used);
            let (part, next) = output.split_at_mut(count);
            part.copy_from_slice(&buffer.bytes[buffer.used..buffer.used + count]);
            output = next;
            buffer.used += count;
        }
        Ok(())
    }

    pub fn byte(&mut self) -> io::Result<u8> {
        let mut byte = [0];
        self.bytes(&mut byte)?;
        Ok(byte[0])
    }

    pub fn unsigned(&mut self) -> io::Result<u64> {
        let mut result = 0;
        for shift in (0..64).step_by(7) {
            let byte = self.byte()?;
            if shift == 63 && byte > 1 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Migrant integer exceeds 64 bits",
                ));
            }
            result |= u64::from(byte & 127) << shift;
            if byte < 128 {
                return Ok(result);
            }
        }
        unreachable!()
    }

    pub fn signed(&mut self) -> io::Result<i64> {
        let value = self.unsigned()?;
        Ok((value >> 1) as i64 ^ -((value & 1) as i64))
    }

    pub fn string(&mut self) -> io::Result<String> {
        let length = self.signed()?;
        let length = usize::try_from(length)
            .ok()
            .filter(|&length| length <= i32::MAX as usize)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid Migrant string length")
            })?;
        let mut bytes = Vec::new();
        self.copy_to(&mut bytes, length)?;
        // .NET's default UTF-8 decoder replaces malformed sequences.
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    pub fn decimal(&mut self) -> io::Result<Decimal> {
        let mut magnitude = [0; 3];
        for word in &mut magnitude {
            *word = self.signed()? as u32;
        }
        let flags = self.signed()? as u32;
        let scale = ((flags >> 16) & 0xff) as u8;
        if scale > 28 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid decimal scale",
            ));
        }
        Ok(Decimal {
            magnitude,
            negative: flags >> 31 != 0,
            scale,
        })
    }

    pub fn guid(&mut self) -> io::Result<Guid> {
        let mut bytes = [0; 16];
        self.bytes(&mut bytes)?;
        Ok(Guid(bytes))
    }

    pub fn date_time(&mut self) -> io::Result<i64> {
        self.time_span()?.checked_add(EPOCH).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "DateTime epoch offset overflow")
        })
    }

    pub fn time_span(&mut self) -> io::Result<i64> {
        let hours = self.byte()?;
        if hours == 255 {
            return self.signed();
        }
        let seconds = self.unsigned()? as u16;
        let days = self.signed()? as i32;
        let total = i64::from(days) * 86_400 + i64::from(hours) * 3600 + i64::from(seconds);
        total
            .checked_mul(SECOND)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "TimeSpan ticks overflow"))
    }

    pub fn finish(mut self) -> io::Result<()> {
        if let Some(buffer) = &mut self.buffer {
            self.stream
                .read_exact(&mut buffer.bytes[..padding(self.physical_position)])?;
        }
        Ok(())
    }

    pub fn copy_to(&mut self, destination: &mut impl Write, mut count: usize) -> io::Result<()> {
        let mut buffer = [0; BLOCK];
        while count != 0 {
            let take = count.min(BLOCK);
            self.bytes(&mut buffer[..take])?;
            destination.write_all(&buffer[..take])?;
            count -= take;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_matches_pinned_migrant_integer_bytes_and_frame_sizes() {
        // Bytes obtained from the pinned C# PrimitiveWriter in Release.
        let prefix = crate::hex_string::decode(
            "010001027F80018101FFFFFFFFFFFFFFFFFF01FEFFFFFFFFFFFFFFFF01FFFFFFFFFFFFFFFFFF01",
            false,
            false,
        )
        .unwrap();
        for (buffered, expected_len) in [(false, 39), (true, 128)] {
            let mut bytes = Vec::new();
            let mut writer = Writer::new(&mut bytes, buffered);
            writer.bytes(&[1]).unwrap();
            for value in [0, -1, 1, -64, 64, -65, i64::MIN, i64::MAX] {
                writer.signed(value).unwrap();
            }
            writer.unsigned(u64::MAX).unwrap();
            writer.finish().unwrap();
            assert_eq!(bytes.len(), expected_len);
            assert_eq!(bytes[..prefix.len()], prefix);
            assert!(bytes[prefix.len()..].iter().all(|&byte| byte == 0));
        }

        for (input, written) in [
            (0, 0),
            (127, 128),
            (128, 128),
            (129, 1024),
            (1024, 1024),
            (1025, 4096),
            (4096, 4096),
            (4097, 8192),
        ] {
            let mut bytes = Vec::new();
            let mut writer = Writer::new(&mut bytes, true);
            writer.bytes(&vec![0xaa; input]).unwrap();
            writer.finish().unwrap();
            assert_eq!(bytes.len(), written);

            let mut stream = std::io::Cursor::new(bytes);
            let mut reader = Reader::new(&mut stream, true);
            let mut values = vec![0; input];
            reader.bytes(&mut values).unwrap();
            reader.finish().unwrap();
            assert!(values.iter().all(|&byte| byte == 0xaa));
            assert_eq!(stream.position(), written as u64);
        }
    }
}

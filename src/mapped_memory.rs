use std::{
    io::{self, Read, Write},
    sync::OnceLock,
};

use crate::migrant::primitive::{Reader, Writer};

const SNAPSHOT_MAGIC: u64 = 0xabcd6366;

const ALIGNMENT: usize = 4096;

#[derive(Clone)]
#[repr(align(4096))]
struct Page([u8; ALIGNMENT]);

/// Lazily allocated, page-aligned mapped segments with stable addresses.
pub struct MappedMemory {
    segments: Vec<OnceLock<Box<[Page]>>>,
    segment_size: usize,
    on_touch: Box<dyn Fn(usize) + Send + Sync>,
}

#[derive(Clone, Copy)]
pub struct MappedSegment<'a> {
    memory: &'a MappedMemory,
    index: usize,
}

impl MappedMemory {
    pub fn new(
        size: usize,
        segment_size: usize,
        on_touch: impl Fn(usize) + Send + Sync + 'static,
    ) -> Option<Self> {
        if size == 0 || segment_size == 0 || size % segment_size != 0 {
            return None;
        }
        Some(Self {
            segments: (0..size / segment_size).map(|_| OnceLock::new()).collect(),
            segment_size,
            on_touch: Box::new(on_touch),
        })
    }

    pub fn write_bytes(&mut self, offset: usize, data: &[u8]) -> io::Result<()> {
        self.check_range(offset, data.len())?;
        let mut address = offset;
        let mut remaining = data;
        while !remaining.is_empty() {
            let segment = address / self.segment_size;
            MappedSegment {
                memory: self,
                index: segment,
            }
            .touch();
            let (page, start, count) = self.page_span(address, remaining.len());
            let (part, tail) = remaining.split_at(count);
            self.segments[segment].get_mut().unwrap()[page].0[start..start + count]
                .copy_from_slice(part);
            address += count;
            remaining = tail;
        }
        Ok(())
    }

    pub fn read_bytes(&self, offset: usize, output: &mut [u8]) -> io::Result<()> {
        self.check_range(offset, output.len())?;
        let mut address = offset;
        let mut remaining = output;
        while !remaining.is_empty() {
            let (page, start, count) = self.page_span(address, remaining.len());
            let (part, tail) = remaining.split_at_mut(count);
            if let Some(pages) = self.segments[address / self.segment_size].get() {
                part.copy_from_slice(&pages[page].0[start..start + count]);
            } else {
                part.fill(0);
            }
            address += count;
            remaining = tail;
        }
        Ok(())
    }

    fn check_range(&self, offset: usize, length: usize) -> io::Result<()> {
        let size = self.segment_size * self.segments.len();
        match offset.checked_add(length) {
            Some(end) if end <= size => Ok(()),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "mapped memory range out of bounds",
            )),
        }
    }

    fn page_span(&self, address: usize, available: usize) -> (usize, usize, usize) {
        let local = address % self.segment_size;
        let start = local % ALIGNMENT;
        let count = available
            .min(ALIGNMENT - start)
            .min(self.segment_size - local);
        (local / ALIGNMENT, start, count)
    }

    pub fn save(&self, stream: impl Write) -> io::Result<()> {
        let mut writer = Writer::new(stream, false);
        writer.unsigned(SNAPSHOT_MAGIC)?;
        writer.signed(self.segment_size as i64)?;
        writer.signed((self.segment_size * self.segments.len()) as i64)?;
        writer.bytes(&[0])?;
        let mut bytes = vec![0; self.segment_size];
        for (index, segment) in self.segments.iter().enumerate() {
            writer.bytes(&[u8::from(segment.get().is_some())])?;
            if segment.get().is_some() {
                self.read_bytes(index * self.segment_size, &mut bytes)?;
                let compressed = lz4_flex::block::compress(&bytes);
                writer.signed(compressed.len() as i64)?;
                writer.bytes(&compressed)?;
            }
        }
        writer.finish()
    }

    pub fn load(stream: impl Read) -> io::Result<Self> {
        fn invalid(message: &'static str) -> io::Error {
            io::Error::new(io::ErrorKind::InvalidData, message)
        }
        let mut reader = Reader::new(stream, false);
        if reader.unsigned()? != SNAPSHOT_MAGIC {
            return Err(invalid("invalid mapped memory snapshot magic"));
        }
        let segment_size = usize::try_from(reader.signed()?)
            .map_err(|_| invalid("invalid mapped memory segment size"))?;
        let size =
            usize::try_from(reader.signed()?).map_err(|_| invalid("invalid mapped memory size"))?;
        if reader.byte()? != 0 {
            return Err(invalid("unsupported mapped memory reset byte"));
        }
        let mut memory = Self::new(size, segment_size, |_| {})
            .ok_or_else(|| invalid("invalid mapped memory dimensions"))?;
        let mut bytes = Vec::new();
        let mut compressed = Vec::new();
        for index in 0..memory.segments.len() {
            match reader.byte()? {
                0 => continue,
                1 => {}
                _ => return Err(invalid("invalid mapped memory segment flag")),
            }
            let length = usize::try_from(reader.signed()?)
                .map_err(|_| invalid("invalid compressed segment size"))?;
            // An LZ4 block needs no more than one extra byte per 255 bytes, plus overhead.
            if length
                > segment_size
                    .saturating_add(segment_size / 255)
                    .saturating_add(32)
            {
                return Err(invalid("compressed mapped memory segment too large"));
            }
            compressed.resize(length, 0);
            reader.bytes(&mut compressed)?;
            bytes.resize(segment_size, 0);
            let decoded = lz4_flex::block::decompress_into(&compressed, &mut bytes)
                .map_err(|_| invalid("invalid compressed mapped memory segment"))?;
            if decoded != segment_size {
                return Err(invalid("incomplete mapped memory segment"));
            }
            memory.write_bytes(index * segment_size, &bytes)?;
        }
        reader.finish()?;
        Ok(memory)
    }

    pub fn mapped_segments(&self) -> impl ExactSizeIterator<Item = MappedSegment<'_>> {
        (0..self.segments.len()).map(|index| MappedSegment {
            memory: self,
            index,
        })
    }
}

impl MappedSegment<'_> {
    pub fn touch(self) {
        let mut allocated = false;
        self.memory.segments[self.index].get_or_init(|| {
            let pages = self.memory.segment_size.div_ceil(ALIGNMENT);
            let data = vec![Page([0; ALIGNMENT]); pages].into_boxed_slice();
            allocated = true;
            data
        });
        if allocated {
            (self.memory.on_touch)(self.index);
        }
    }

    /// The address is valid for this segment's lifetime after it has been touched.
    pub fn pointer(self) -> Option<*const u8> {
        self.memory.segments[self.index]
            .get()
            .map(|pages| pages[0].0.as_ptr())
    }
}

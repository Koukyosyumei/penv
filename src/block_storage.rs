use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::Path,
};

/// File-backed block storage whose unallocated tail reads as zero.
pub struct BlockStorage {
    file: File,
    block_size: u64,
    capacity: u64,
}

impl BlockStorage {
    pub fn open(path: &Path, block_count: u32, block_size: u32) -> io::Result<Self> {
        if block_size == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "block size must be positive",
            ));
        }
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        Ok(Self {
            file,
            block_size: block_size.into(),
            capacity: u64::from(block_count) * u64::from(block_size),
        })
    }

    fn offset(&self, start: u32, count: u32) -> io::Result<(u64, u64)> {
        let start = u64::from(start) * self.block_size;
        let size = u64::from(count) * self.block_size;
        if start > self.capacity || size > self.capacity - start {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "block range outside capacity",
            ));
        }
        Ok((start, size))
    }

    pub fn read(&mut self, start: u32, count: u32) -> io::Result<Vec<u8>> {
        let (offset, size) = self.offset(start, count)?;
        let size = usize::try_from(size).map_err(|_| io::ErrorKind::InvalidInput)?;
        let mut data = Vec::new();
        data.try_reserve_exact(size).map_err(io::Error::other)?;
        data.resize(size, 0);
        let length = self.file.metadata()?.len();
        read_sparse(&mut self.file, length, offset, &mut data)?;
        Ok(data)
    }

    pub fn write(&mut self, start: u32, count: u32, data: &[u8]) -> io::Result<()> {
        let (offset, size) = self.offset(start, count)?;
        if data.len() as u64 > size {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "write exceeds requested blocks",
            ));
        }
        if !data.is_empty() {
            self.file.seek(SeekFrom::Start(offset))?;
            self.file.write_all(data)?;
        }
        Ok(())
    }
}

pub(crate) fn read_sparse(
    file: &mut File,
    stored_length: u64,
    offset: u64,
    buffer: &mut [u8],
) -> io::Result<()> {
    let stored = stored_length
        .saturating_sub(offset)
        .min(buffer.len() as u64) as usize;
    if stored != 0 {
        file.seek(SeekFrom::Start(offset))?;
        file.read_exact(&mut buffer[..stored])?;
    }
    buffer[stored..].fill(0);
    Ok(())
}

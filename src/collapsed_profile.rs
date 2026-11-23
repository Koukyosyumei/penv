use std::{
    fs::File,
    io::{self, BufWriter, Write},
    path::Path,
};

pub struct CollapsedProfile {
    file: BufWriter<File>,
    flush_immediately: bool,
    limit: Option<u64>,
    written: u64,
    stack: Vec<(u32, Option<u32>)>,
    instructions: u64,
}

impl CollapsedProfile {
    pub fn new(
        path: &Path,
        root: u32,
        flush_immediately: bool,
        limit: Option<u64>,
    ) -> io::Result<Self> {
        Ok(Self {
            file: BufWriter::new(File::create(path)?),
            flush_immediately,
            limit,
            written: 0,
            stack: vec![(root, None)],
            instructions: 0,
        })
    }

    pub fn retired(
        &mut self,
        pc: u32,
        instruction: Option<(u16, u16)>,
        next: u32,
    ) -> io::Result<bool> {
        self.instructions += 1;
        let call_width = match instruction {
            Some((high @ 0xf000..=0xf7ff, low))
                if high & 0xf800 == 0xf000 && low & 0xd000 == 0xd000 =>
            {
                Some(4)
            }
            Some((instruction, _)) if instruction & 0xff87 == 0x4780 => Some(2),
            _ => None,
        };
        if let Some(width) = call_width {
            if !self.flush_stack()? {
                return Ok(false);
            }
            self.stack.push((next, Some(pc.wrapping_add(width))));
        } else if let Some((instruction, _)) = instruction
            && (instruction == 0x4770 || instruction & 0xff00 == 0xbd00)
            && let Some(index) = self
                .stack
                .iter()
                .rposition(|&(_, saved)| saved == Some(next))
        {
            if !self.flush_stack()? {
                return Ok(false);
            }
            self.stack.truncate(index);
        }
        Ok(true)
    }

    fn flush_stack(&mut self) -> io::Result<bool> {
        if self.instructions == 0 {
            return Ok(true);
        }
        let mut line = Vec::new();
        for (index, (address, _)) in self.stack.iter().enumerate() {
            if index != 0 {
                line.push(b';');
            }
            write!(&mut line, "{address:#010x}")?;
        }
        writeln!(&mut line, " {}", self.instructions)?;
        let count = line.len() as u64;
        if self
            .limit
            .is_some_and(|limit| count > limit.saturating_sub(self.written))
        {
            eprintln!("Profiler: Maximum file size exceeded, removing profiler");
            self.instructions = 0;
            self.file.flush()?;
            return Ok(false);
        }
        self.file.write_all(&line)?;
        self.written += count;
        self.instructions = 0;
        if self.flush_immediately {
            self.file.flush()?;
        }
        Ok(true)
    }

    pub fn finish(mut self) -> io::Result<()> {
        self.flush_stack()?;
        self.file.flush()
    }
}

use std::sync::OnceLock;

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

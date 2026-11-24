use std::io::{Cursor, Seek, SeekFrom};

use crate::mapped_memory::MappedMemory;

use super::migrant::Random;

#[test]
fn serialize_memory() {
    const SIZE: usize = 128 * 1024 * 1024;
    const COUNT: usize = 30;
    const LENGTH: usize = 100 * 1024;

    let mut rng = Random(0x172b_f045);
    let mut buffers: Vec<(usize, Vec<u8>)> = Vec::with_capacity(COUNT);
    for _ in 0..COUNT {
        let buffer = (0..LENGTH).map(|_| rng.next(256) as u8).collect();
        let address = loop {
            let address = rng.next((SIZE - LENGTH) as i32) as usize;
            if buffers
                .iter()
                .all(|(previous, _)| previous.abs_diff(address) > LENGTH)
            {
                break address;
            }
        };
        buffers.push((address, buffer));
    }

    let mut stream = Cursor::new(Vec::new());
    {
        // Upstream chooses a segment size of size / 16 for this memory.
        let mut memory = MappedMemory::new(SIZE, SIZE / 16, |_| {}).unwrap();
        for (address, buffer) in &buffers {
            memory.write_bytes(*address, buffer).unwrap();
        }
        memory.save(&mut stream).unwrap();
    }

    stream.seek(SeekFrom::Start(0)).unwrap();
    let memory = MappedMemory::load(stream).unwrap();
    for (address, buffer) in buffers {
        let mut copy = vec![0; LENGTH];
        memory.read_bytes(address, &mut copy).unwrap();
        assert_eq!(copy, buffer, "buffer at {address:#x}");
    }
}

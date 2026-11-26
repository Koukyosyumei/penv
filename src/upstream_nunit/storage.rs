use crate::block_storage::BlockStorage;
use std::{fs, path::PathBuf};

struct FileGuard(PathBuf);

impl Drop for FileGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[test]
fn should_read_and_write_lba_backend() {
    const BLOCK: u32 = 512;
    const MAX_BLOCKS: u32 = 1024 * 256;
    const TEST_BLOCKS: usize = 500;
    let mut state = 0xb56f_af9b_2539_7f37_u64;
    let mut random = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state & i32::MAX as u64) as u32
    };

    for iteration in 0..10 {
        let file = FileGuard(
            std::env::temp_dir().join(format!("penv-lba-{}-{iteration}", std::process::id())),
        );
        fs::File::create_new(&file.0).unwrap();
        let block_count = random() % (MAX_BLOCKS + 1);
        let mut storage = BlockStorage::open(&file.0, block_count, BLOCK).unwrap();
        let mut positions: Vec<_> = (0..block_count).map(|block| (random(), block)).collect();
        positions.sort_unstable();
        let count = TEST_BLOCKS.min(block_count as usize);
        let mut contents = Vec::with_capacity(count);

        for &(_, position) in &positions[..count] {
            let bytes: Vec<_> = (0..BLOCK).map(|_| random() as u8).collect();
            storage.write(position, 1, &bytes).unwrap();
            contents.push(bytes);
        }
        for (&(_, position), expected) in positions[..count].iter().zip(contents) {
            assert_eq!(storage.read(position, 1).unwrap(), expected);
        }
    }
}

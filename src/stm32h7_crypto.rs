use std::collections::VecDeque;

use aes::{
    Aes128, Aes192, Aes256,
    cipher::{BlockDecrypt, BlockEncrypt, BlockSizeUser, KeyInit, consts::U16},
};

const ENABLE: u32 = 1 << 15;
const FIFO_DEPTH: usize = 8;
const BLOCK_WORDS: usize = 4;

struct Gcm {
    hash_key: u128,
    counter: [u8; 16],
    tag_mask: u128,
    header: GhashStream,
    payload: GhashStream,
}

#[derive(Default)]
struct GhashStream {
    hash: u128,
    last: Option<[u8; 16]>,
    blocks: u64,
}

impl GhashStream {
    fn push(&mut self, key: u128, block: [u8; 16]) {
        if let Some(previous) = self.last.replace(block) {
            self.hash = ghash_multiply(self.hash ^ u128::from_be_bytes(previous), key);
        }
        self.blocks += 1;
    }

    fn finish(self, key: u128, bits: u64) -> Option<u128> {
        if bits % 8 != 0 || bits.div_ceil(128) != self.blocks {
            return None;
        }
        let Some(mut last) = self.last else {
            return Some(0);
        };
        let valid_bytes = ((bits - 1) % 128 / 8 + 1) as usize;
        last[valid_bytes..].fill(0);
        Some(ghash_multiply(self.hash ^ u128::from_be_bytes(last), key))
    }
}

fn ghash_multiply(left: u128, mut right: u128) -> u128 {
    let mut product = 0;
    for bit in (0..128).rev() {
        if left & (1 << bit) != 0 {
            product ^= right;
        }
        let low = right & 1;
        right >>= 1;
        if low != 0 {
            right ^= 0xe1 << 120;
        }
    }
    product
}

impl Gcm {
    fn finish(self, lengths: [u8; 16]) -> Option<[u8; 16]> {
        let key = self.hash_key;
        let header_bits = u64::from_be_bytes(lengths[..8].try_into().unwrap());
        let payload_bits = u64::from_be_bytes(lengths[8..].try_into().unwrap());
        let mut header = self.header.finish(key, header_bits)?;
        let mut blocks = self.payload.blocks;
        let payload = self.payload.finish(key, payload_bits)?;
        // Shift the AAD contribution forward by the number of ciphertext blocks.
        let mut power = key;
        while blocks != 0 {
            if blocks & 1 != 0 {
                header = ghash_multiply(header, power);
            }
            blocks >>= 1;
            if blocks != 0 {
                power = ghash_multiply(power, power);
            }
        }
        let tag =
            ghash_multiply(header ^ payload ^ u128::from_be_bytes(lengths), key) ^ self.tag_mask;
        Some(tag.to_be_bytes())
    }
}

pub struct Crypto {
    pub base: u32,
    pub irq: Option<u8>,
    control: u32,
    dma_control: u32,
    mask: u32,
    key_and_iv: [u32; 12],
    input: VecDeque<u32>,
    output: VecDeque<u32>,
    gcm: Option<Gcm>,
}

impl Crypto {
    pub const MODEL: &str = "Miscellaneous.Crypto.STM32H7_CRYPTO";
    pub const SIZE: u32 = 0x400;

    pub fn new(base: u32, irq: Option<u8>) -> Self {
        Self {
            base,
            irq,
            control: 0,
            dma_control: 0,
            mask: 0,
            key_and_iv: [0; 12],
            input: VecDeque::new(),
            output: VecDeque::new(),
            gcm: None,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new(self.base, self.irq);
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    fn raw_interrupt(&self) -> u32 {
        u32::from(self.input.len() < FIFO_DEPTH) | (u32::from(!self.output.is_empty()) << 1)
    }

    pub fn interrupt(&self) -> Option<u8> {
        self.irq.filter(|_| self.raw_interrupt() & self.mask != 0)
    }

    fn swap(&self, word: u32) -> u32 {
        match (self.control >> 6) & 3 {
            0 => word,
            1 => word.rotate_left(16),
            2 => word.swap_bytes(),
            _ => word.reverse_bits(),
        }
    }

    pub fn read(&mut self, address: u32) -> u32 {
        match address - self.base {
            0 => self.control,
            4 => {
                u32::from(self.input.is_empty())
                    | (self.raw_interrupt() << 1)
                    | (u32::from(self.output.len() == FIFO_DEPTH) << 3)
            }
            8 => self.input.pop_front().unwrap_or(0),
            0xc => {
                let result = self.output.pop_front().unwrap_or(0);
                self.process();
                self.swap(result)
            }
            0x10 => self.dma_control,
            0x14 => self.mask,
            0x18 => self.raw_interrupt(),
            0x1c => self.raw_interrupt() & self.mask,
            offset @ 0x20..=0x4c => self.key_and_iv[((offset - 0x20) / 4) as usize],
            _ => 0,
        }
    }

    pub fn write(&mut self, address: u32, value: u32) {
        match address - self.base {
            0 => {
                if value & (1 << 14) != 0 {
                    self.input.clear();
                    self.output.clear();
                }
                self.control = value & 0x000b_83fc;
                if self.control & ENABLE != 0 && self.mode() == 8 && self.gcm_phase() == 0 {
                    self.initialize_gcm();
                    self.control &= !ENABLE;
                } else if self.mode() == 7 {
                    // Key preparation completes immediately.
                    self.control &= !ENABLE;
                }
            }
            8 if self.input.len() < FIFO_DEPTH => self.input.push_back(self.swap(value)),
            0x10 => self.dma_control = value & 3,
            0x14 => self.mask = value & 3,
            offset @ 0x20..=0x4c => self.key_and_iv[((offset - 0x20) / 4) as usize] = value,
            _ => {}
        }
        self.process();
    }

    fn mode(&self) -> u32 {
        ((self.control >> 3) & 7) | ((self.control >> 16) & 8)
    }

    fn gcm_phase(&self) -> u32 {
        (self.control >> 16) & 3
    }

    fn encrypt_gcm_block(&self, block: &mut [u8; 16]) -> bool {
        let mut key = [0; 32];
        for (word, bytes) in self.key_and_iv[..8].iter().zip(key.chunks_exact_mut(4)) {
            bytes.copy_from_slice(&word.to_be_bytes());
        }
        match (self.control >> 8) & 3 {
            0 => Aes128::new((&key[16..]).into()).encrypt_block(block.into()),
            1 => Aes192::new((&key[8..]).into()).encrypt_block(block.into()),
            2 => Aes256::new((&key).into()).encrypt_block(block.into()),
            _ => return false,
        }
        true
    }

    fn initialize_gcm(&mut self) {
        let mut hash_key = [0; 16];
        if !self.encrypt_gcm_block(&mut hash_key) {
            self.gcm = None;
            return;
        }
        let mut counter = [0; 16];
        for (word, bytes) in self.key_and_iv[8..11]
            .iter()
            .zip(counter.chunks_exact_mut(4))
        {
            bytes.copy_from_slice(&word.to_be_bytes());
        }
        counter[15] = 1;
        let mut tag_mask = counter;
        self.encrypt_gcm_block(&mut tag_mask);
        counter[15] = 2;
        self.gcm = Some(Gcm {
            hash_key: u128::from_be_bytes(hash_key),
            counter,
            tag_mask: u128::from_be_bytes(tag_mask),
            header: GhashStream::default(),
            payload: GhashStream::default(),
        });
    }

    fn process_gcm(&mut self) {
        let phase = self.gcm_phase();
        while self.gcm.is_some()
            && self.input.len() >= BLOCK_WORDS
            && (phase == 1 || self.output.len() <= FIFO_DEPTH - BLOCK_WORDS)
        {
            let mut block = [0; 16];
            for (word, bytes) in self
                .input
                .drain(..BLOCK_WORDS)
                .zip(block.chunks_exact_mut(4))
            {
                bytes.copy_from_slice(&word.to_be_bytes());
            }
            let mut state = self.gcm.take().unwrap();
            match phase {
                1 => {
                    state.header.push(state.hash_key, block);
                }
                2 => {
                    let decrypt = self.control & 4 != 0;
                    if decrypt {
                        state.payload.push(state.hash_key, block);
                    }
                    let mut mask = state.counter;
                    if !self.encrypt_gcm_block(&mut mask) {
                        return;
                    }
                    xor_block(&mut block, &mask);
                    if !decrypt {
                        state.payload.push(state.hash_key, block);
                    }
                    let counter = u32::from_be_bytes(state.counter[12..].try_into().unwrap());
                    state.counter[12..].copy_from_slice(&counter.wrapping_add(1).to_be_bytes());
                    self.push_gcm_output(block);
                }
                3 => {
                    if let Some(tag) = state.finish(block) {
                        self.push_gcm_output(tag);
                    }
                    return;
                }
                _ => return,
            }
            self.gcm = Some(state);
        }
    }

    fn push_gcm_output(&mut self, block: [u8; 16]) {
        self.output.extend(
            block
                .chunks_exact(4)
                .map(|bytes| u32::from_be_bytes(bytes.try_into().unwrap())),
        );
    }

    fn process(&mut self) {
        if self.mode() == 8 {
            if self.control & ENABLE != 0 {
                self.process_gcm();
            }
            return;
        }
        let key_size = (self.control >> 8) & 3;
        while self.control & ENABLE != 0
            && matches!(self.mode(), 4..=6)
            && key_size < 3
            && self.input.len() >= BLOCK_WORDS
            && self.output.len() <= FIFO_DEPTH - BLOCK_WORDS
        {
            let mut key = [0; 32];
            for (value, bytes) in self.key_and_iv[..8].iter().zip(key.chunks_exact_mut(4)) {
                bytes.copy_from_slice(&value.to_be_bytes());
            }
            let mut block = [0; 16];
            for (value, bytes) in self
                .input
                .drain(..BLOCK_WORDS)
                .zip(block.chunks_exact_mut(4))
            {
                bytes.copy_from_slice(&value.to_be_bytes());
            }
            let decrypt = self.control & 4 != 0;
            let mode = self.mode();
            let mut iv = [0; 16];
            for (word, bytes) in self.key_and_iv[8..].iter().zip(iv.chunks_exact_mut(4)) {
                bytes.copy_from_slice(&word.to_be_bytes());
            }
            match key_size {
                0 => transform(
                    Aes128::new((&key[16..]).into()),
                    &mut block,
                    decrypt,
                    mode,
                    &mut iv,
                ),
                1 => transform(
                    Aes192::new((&key[8..]).into()),
                    &mut block,
                    decrypt,
                    mode,
                    &mut iv,
                ),
                _ => transform(
                    Aes256::new((&key).into()),
                    &mut block,
                    decrypt,
                    mode,
                    &mut iv,
                ),
            }
            for (word, bytes) in self.key_and_iv[8..].iter_mut().zip(iv.chunks_exact(4)) {
                *word = u32::from_be_bytes(bytes.try_into().unwrap());
            }
            self.output.extend(
                block
                    .chunks_exact(4)
                    .map(|bytes| u32::from_be_bytes(bytes.try_into().unwrap())),
            );
        }
    }
}

fn transform(
    cipher: impl BlockEncrypt + BlockDecrypt + BlockSizeUser<BlockSize = U16>,
    block: &mut [u8; 16],
    decrypt: bool,
    mode: u32,
    iv: &mut [u8; 16],
) {
    match (mode, decrypt) {
        (5, false) => {
            xor_block(block, iv);
            cipher.encrypt_block(block.into());
            *iv = *block;
        }
        (5, true) => {
            let next_iv = *block;
            cipher.decrypt_block(block.into());
            xor_block(block, iv);
            *iv = next_iv;
        }
        (6, _) => {
            let mut mask = *iv;
            cipher.encrypt_block((&mut mask).into());
            xor_block(block, &mask);
            let counter = u32::from_be_bytes(iv[12..].try_into().unwrap());
            iv[12..].copy_from_slice(&counter.wrapping_add(1).to_be_bytes());
        }
        (_, true) => cipher.decrypt_block(block.into()),
        (_, false) => cipher.encrypt_block(block.into()),
    }
}

fn xor_block(block: &mut [u8; 16], mask: &[u8; 16]) {
    for (byte, key) in block.iter_mut().zip(mask) {
        *byte ^= key;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aes_gcm_partial_payload_uses_final_bit_length() {
        let mut crypto = Crypto::new(0, None);
        let control = ENABLE | (1 << 19);
        crypto.write(0x4c, 2);
        crypto.write(0, control);
        crypto.write(0, control | (2 << 16));
        for word in [0u32, 0x000000ff, 0xffffffff, 0xffffffff] {
            crypto.write(8, word);
        }
        let output: [u32; 4] = std::array::from_fn(|_| crypto.read(0xc));
        assert_eq!(output[0], 0x0388dace);
        assert_eq!(output[1] >> 8, 0x60b6a3);

        crypto.write(0, control | (3 << 16));
        for word in [0, 0, 0, 56] {
            crypto.write(8, word);
        }
        assert_eq!(
            std::array::from_fn(|_| crypto.read(0xc)),
            [0xf191b0a7, 0x0de636f1, 0x48bed5be, 0xa5b55615]
        );
    }

    #[test]
    fn aes_gcm_partial_header_and_multiblock_decrypt() {
        let mut crypto = Crypto::new(0, Some(79));
        let control = ENABLE | (1 << 19) | 4;
        crypto.write(0x4c, 2);
        crypto.write(0x14, 2);
        crypto.write(0, control);
        crypto.write(0, control | (1 << 16));
        for word in [0x68656164, 0x6572ffff, 0xffffffff, 0xffffffff] {
            crypto.write(8, word);
        }
        crypto.write(0, control | (2 << 16));
        for input in [
            [0x0388dace, 0x60b6a392, 0xf328c2b9, 0x71b2fe78],
            [0xf795aaff, 0xffffffff, 0xffffffff, 0xffffffff],
        ] {
            for word in input {
                crypto.write(8, word);
            }
            assert_eq!(crypto.interrupt(), Some(79));
            let output: [u32; 4] = std::array::from_fn(|_| crypto.read(0xc));
            assert_eq!(output[0] >> 8, 0);
        }
        crypto.write(0, control | (3 << 16));
        for word in [0, 48, 0, 152] {
            crypto.write(8, word);
        }
        assert_eq!(
            std::array::from_fn(|_| crypto.read(0xc)),
            [0xa3148393, 0x54956b13, 0x30a077d8, 0x807e35c7]
        );
    }

    fn aes_gcm_fixture(decrypt: bool, use_interrupt: bool) {
        let mut crypto = Crypto::new(0, Some(79));
        let ciphertext = [0x0388dace, 0x60b6a392, 0xf328c2b9, 0x71b2fe78];
        let input = if decrypt { ciphertext } else { [0; 4] };
        let expected = if decrypt { [0; 4] } else { ciphertext };
        let control = ENABLE | (1 << 19) | (u32::from(decrypt) << 2);
        crypto.write(0x4c, 2);
        crypto.write(0x14, if use_interrupt { 2 } else { 0 });
        crypto.write(0, control);
        assert_eq!(crypto.read(0) & ENABLE, 0);
        crypto.write(0, control | (1 << 16));
        crypto.write(0, control | (2 << 16));
        for word in input {
            crypto.write(8, word);
        }
        assert_ne!(crypto.read(4) & (1 << 2), 0);
        assert_eq!(crypto.interrupt(), use_interrupt.then_some(79));
        assert_eq!(std::array::from_fn(|_| crypto.read(0xc)), expected);
        assert_eq!(crypto.interrupt(), None);
        crypto.write(0, control | (3 << 16));
        for word in [0, 0, 0, 128] {
            crypto.write(8, word);
        }
        assert_eq!(crypto.interrupt(), use_interrupt.then_some(79));
        assert_eq!(
            std::array::from_fn(|_| crypto.read(0xc)),
            [0xab6e47d4, 0x2cec13bd, 0xf53a67b2, 0x1257bddf]
        );
        assert_eq!(crypto.interrupt(), None);
    }

    #[test]
    fn aes_gcm_aligned_polling_encrypt_and_tag() {
        aes_gcm_fixture(false, false);
    }

    #[test]
    fn aes_gcm_aligned_interrupt_decrypt_and_tag() {
        aes_gcm_fixture(true, true);
    }

    #[test]
    fn aes_128_ecb_round_trip_and_output_fifo_irq() {
        let mut crypto = Crypto::new(0, Some(79));
        let plain = [0x00112233, 0x44556677, 0x8899aabb, 0xccddeeff];
        let cipher = [0x69c4e0d8, 0x6a7b0430, 0xd8cdb780, 0x70b4c55a];
        for (offset, word) in [0x00010203, 0x04050607, 0x08090a0b, 0x0c0d0e0f]
            .into_iter()
            .enumerate()
        {
            crypto.write(0x30 + offset as u32 * 4, word);
        }
        crypto.write(0x14, 2);
        for (input, expected, direction) in [(plain, cipher, 0), (cipher, plain, 4)] {
            crypto.write(0, ENABLE | (4 << 3) | direction);
            for word in input {
                crypto.write(8, word);
            }
            assert_eq!(crypto.interrupt(), Some(79));
            assert_eq!(std::array::from_fn(|_| crypto.read(0xc)), expected);
            assert_eq!(crypto.interrupt(), None);
        }
    }
}

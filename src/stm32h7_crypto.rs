use std::collections::VecDeque;

use aes::{
    Aes128, Aes192, Aes256,
    cipher::{BlockDecrypt, BlockEncrypt, BlockSizeUser, KeyInit, consts::U16},
};

const ENABLE: u32 = 1 << 15;
const FIFO_DEPTH: usize = 8;
const BLOCK_WORDS: usize = 4;

pub struct Crypto {
    pub base: u32,
    pub irq: Option<u8>,
    control: u32,
    dma_control: u32,
    mask: u32,
    key_and_iv: [u32; 12],
    input: VecDeque<u32>,
    output: VecDeque<u32>,
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
                if self.mode() == 7 {
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

    fn process(&mut self) {
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

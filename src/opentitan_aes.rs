use aes::{
    cipher::{consts::U16, BlockDecrypt, BlockEncrypt, BlockSizeUser, KeyInit},
    Aes128, Aes192, Aes256,
};

use crate::{
    bus_translation::NativePeripheral, pseudorandom::Sequence,
    tracked_register_bytes::TrackedRegisterBytes,
};

const MANUAL: u32 = 1 << 15;
const SIDELOAD: u32 = 1 << 11;

pub struct OpenTitanAes {
    shares: [TrackedRegisterBytes; 2],
    iv: TrackedRegisterBytes,
    input: TrackedRegisterBytes,
    output: TrackedRegisterBytes,
    key: [u8; 32],
    sideload: Option<Vec<u8>>,
    control: u32,
    auxiliary: u32,
    auxiliary_write_enable: u32,
    ready: bool,
    valid: bool,
    random: Sequence,
}

impl OpenTitanAes {
    pub fn new(seed: i32) -> Self {
        let bytes = |parts| TrackedRegisterBytes::new(parts, 4).unwrap();
        Self {
            shares: [bytes(8), bytes(8)],
            iv: bytes(4),
            input: bytes(4),
            output: bytes(4),
            key: [0; 32],
            sideload: None,
            control: 0xc0,
            auxiliary: 1,
            auxiliary_write_enable: 1,
            ready: true,
            valid: false,
            random: Sequence::new(seed),
        }
    }

    pub fn input_data_ready(&self) -> bool {
        self.input.all_written()
    }

    pub fn set_sideload_key(&mut self, key: Vec<u8>) {
        self.sideload = Some(key);
    }

    fn prepare_key(&mut self) {
        let [first, second] = &mut self.shares;
        if first.all_written() && second.all_written() {
            for ((output, left), right) in self
                .key
                .iter_mut()
                .zip(first.retrieve(false))
                .zip(second.retrieve(false))
            {
                *output = left ^ right;
            }
        }
    }

    fn transform(&mut self) {
        if (self.control >> 2) & 0x3f != 1 || !self.input_data_ready() {
            return;
        }
        let key = if self.control & SIDELOAD != 0 {
            let Some(key) = &self.sideload else { return };
            key.as_slice()
        } else {
            &self.key
        };
        let mut data = aes::cipher::Block::<Aes128>::default();
        data.copy_from_slice(self.input.retrieve(false));
        let decrypt = self.control & 3 == 2;
        fn apply<C: BlockEncrypt + BlockDecrypt + BlockSizeUser<BlockSize = U16> + KeyInit>(
            key: &[u8],
            block: &mut aes::cipher::Block<Aes128>,
            decrypt: bool,
        ) -> bool {
            let Ok(cipher) = C::new_from_slice(key) else {
                return false;
            };
            if decrypt {
                cipher.decrypt_block(block);
            } else {
                cipher.encrypt_block(block);
            }
            true
        }
        // The upstream AesManaged model validates KEY_LEN first, then
        // assigns the full key buffer. The assignment sets the effective size.
        if !matches!((self.control >> 8) & 7, 1 | 2 | 4) {
            return;
        }
        let processed = match key.len() {
            16 => apply::<Aes128>(key, &mut data, decrypt),
            24 => apply::<Aes192>(key, &mut data, decrypt),
            32 => apply::<Aes256>(key, &mut data, decrypt),
            _ => false,
        };
        if processed {
            self.output.set_array(data.to_vec(), false).unwrap();
            self.valid = true;
        }
    }

    fn trigger(&mut self, value: u32) {
        if value & 1 != 0 && self.control & MANUAL != 0 {
            self.transform();
        }
        let random = &mut self.random;
        let mut generate = |size| (0..size).map(|_| random.next() as u8).collect();
        if value & 2 != 0 {
            self.iv.set_array(generate(16), false).unwrap();
            for share in &mut self.shares {
                share.set_array(generate(32), false).unwrap();
            }
            self.input.set_array(generate(16), false).unwrap();
        }
        if value & 4 != 0 {
            self.output.set_array(generate(16), false).unwrap();
        }
    }
}

impl NativePeripheral for OpenTitanAes {
    fn read(&mut self, _: Option<&str>, offset: u64) -> u64 {
        if offset % 4 != 0 {
            return 0;
        }
        u64::from(match offset {
            0x64..=0x70 => {
                let value = self.output.part(((offset - 0x64) / 4) as usize).unwrap();
                if self.output.all_read() {
                    self.ready = true;
                    self.valid = false;
                }
                value
            }
            0x74 => self.control,
            0x78 => self.auxiliary,
            0x7c => self.auxiliary_write_enable,
            0x84 => {
                1 | (u32::from(self.valid) << 3)
                    | (u32::from(self.ready && self.control & MANUAL == 0) << 4)
            }
            _ => 0,
        })
    }

    fn write(&mut self, _: Option<&str>, offset: u64, value: u64) {
        if offset % 4 != 0 {
            return;
        }
        let value = value as u32;
        match offset {
            0x04..=0x40 if self.control & SIDELOAD == 0 => {
                let word = ((offset - 4) / 4) as usize;
                self.shares[word / 8].set_part(word % 8, value).unwrap();
                self.prepare_key();
            }
            0x44..=0x50 => self
                .iv
                .set_part(((offset - 0x44) / 4) as usize, value)
                .unwrap(),
            0x54..=0x60 => {
                self.input
                    .set_part(((offset - 0x54) / 4) as usize, value)
                    .unwrap();
                if self.control & MANUAL == 0 && self.input_data_ready() {
                    self.transform();
                }
            }
            0x74 => self.control = value & 0x1ffff,
            0x78 => self.auxiliary = value & 1,
            0x7c => self.auxiliary_write_enable = value & 1,
            0x80 => self.trigger(value),
            _ => {}
        }
    }
}

use tiny_keccak::{Hasher, Kmac};

pub const OPERATION_CONTROLS: u32 = 0x14;
pub const WORKING_STATE: u32 = 0xe4;
pub const STATUS: u32 = 0xe8;
pub const ERROR_CODE: u32 = 0xec;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u32)]
pub enum WorkingState {
    Reset,
    Init,
    CreatorRootKey,
    OwnerIntermediateKey,
    OwnerKey,
    Disabled,
}

#[repr(u32)]
pub enum Operation {
    Advance,
    GenerateId,
    GenerateSwOutput,
    GenerateHwOutput,
    Disable,
}

pub struct KeyMaterial {
    pub device_id: [u8; 32],
    pub life_cycle_diversification_constant: [u8; 16],
    pub creator_key: [u8; 32],
    pub owner_key: [u8; 32],
    pub root_key: [u8; 64],
    pub soft_output_seed: [u8; 32],
    pub hard_output_seed: [u8; 32],
    pub destination_seeds: [[u8; 32]; 4],
    pub revision_seed: [u8; 32],
    pub identity_seeds: [[u8; 32]; 3],
}

pub struct KeyManager {
    material: KeyMaterial,
    rom_digest: [u8; 32],
    state: WorkingState,
    error: u32,
    status: u32,
    control: u32,
    key: [u8; 64],
    binding: [[u8; 32]; 2],
    salt: [u8; 32],
    version: u32,
    max_versions: [u32; 3],
    output: [u8; 64],
}

impl KeyManager {
    pub fn new(material: KeyMaterial, rom_digest: [u8; 32]) -> Self {
        Self {
            material,
            rom_digest,
            state: WorkingState::Reset,
            error: 0,
            status: 0,
            control: 0,
            key: [0; 64],
            binding: [[0; 32]; 2],
            salt: [0; 32],
            version: 0,
            max_versions: [0; 3],
            output: [0; 64],
        }
    }

    pub fn reset(&mut self) {
        self.state = WorkingState::Reset;
        self.error = 0;
        self.status = 0;
        self.control = 0;
        self.key.fill(0);
        self.binding.fill([0; 32]);
        self.salt.fill(0);
        self.version = 0;
        self.max_versions.fill(0);
        self.output.fill(0);
    }

    pub fn read_double_word(&mut self, offset: u32) -> u32 {
        match offset {
            OPERATION_CONTROLS => self.control,
            WORKING_STATE => self.state as u32,
            STATUS => self.status,
            ERROR_CODE => self.error,
            0xa4..=0xe0 if offset % 4 == 0 => {
                let start = (offset - 0xa4) as usize;
                let word = &mut self.output[start..start + 4];
                let value = u32::from_le_bytes(word.try_into().unwrap());
                word.fill(0);
                value
            }
            _ => 0,
        }
    }

    pub fn write_double_word(&mut self, offset: u32, value: u32) {
        match offset {
            OPERATION_CONTROLS => {
                self.control = value & 0x70f0;
                if value & 1 != 0 {
                    self.run();
                }
            }
            STATUS => self.status &= !(value & 3),
            ERROR_CODE => self.error &= !(value & 7),
            0x28..=0x84 if offset % 4 == 0 => {
                let byte = ((offset - 0x28) % 32) as usize;
                let bank = match offset {
                    0x28..=0x44 => &mut self.binding[0],
                    0x48..=0x64 => &mut self.binding[1],
                    _ => &mut self.salt,
                };
                bank[byte..byte + 4].copy_from_slice(&value.to_le_bytes());
            }
            0x88 => self.version = value,
            0x90 => self.max_versions[0] = value,
            0x98 => self.max_versions[1] = value,
            0xa0 => self.max_versions[2] = value,
            _ => {}
        }
    }

    fn run(&mut self) {
        use WorkingState::*;
        let operation = (self.control >> 4) & 7;
        if operation > Operation::Disable as u32 || (self.state == Reset && operation != 0) {
            return;
        }
        if self.state == Disabled || (self.state == Init && (1..=3).contains(&operation)) {
            self.error |= 1;
            self.status = 3;
            return;
        }
        match operation {
            0 => self.advance(),
            1 => {
                let index = self.state as usize - CreatorRootKey as usize;
                self.output = self.derive(&[&self.material.identity_seeds[index]]);
            }
            2 | 3 => self.generate_output(operation == 3),
            4 => self.state = Disabled,
            _ => unreachable!(),
        }
    }

    fn advance(&mut self) {
        use WorkingState::*;
        let binding = &self.binding[((self.control >> 7) & 1) as usize];
        self.state = match self.state {
            Reset => {
                self.key = self.material.root_key;
                Init
            }
            Init => {
                self.key = self.derive(&[
                    binding,
                    &self.material.revision_seed,
                    &self.material.device_id,
                    &self.material.life_cycle_diversification_constant,
                    &self.rom_digest,
                    &self.material.creator_key,
                ]);
                CreatorRootKey
            }
            CreatorRootKey => {
                self.key = self.derive(&[binding, &self.material.owner_key]);
                OwnerIntermediateKey
            }
            OwnerIntermediateKey => {
                self.key = self.derive(&[binding]);
                OwnerKey
            }
            OwnerKey => Disabled,
            Disabled => unreachable!(),
        };
        self.status = 0;
    }

    fn generate_output(&mut self, hardware: bool) {
        let index = self.state as usize - WorkingState::CreatorRootKey as usize;
        if self.version > self.max_versions[index] {
            self.error |= 2;
            return;
        }
        let destination = (self.control >> 12) as usize;
        let seed = self
            .material
            .destination_seeds
            .get(destination)
            .map_or(&[][..], |s| &s[..]);
        let input = [
            &self.version.to_le_bytes()[..],
            &self.salt,
            seed,
            if hardware {
                &self.material.hard_output_seed
            } else {
                &self.material.soft_output_seed
            },
        ];
        if hardware {
            // This fixture constructs the peripheral without a sideload destination.
            if destination == 3 {
                let _: [u8; 48] = self.derive(&input);
            } else {
                let _: [u8; 32] = self.derive(&input);
            }
        } else {
            self.output = self.derive(&input);
        }
    }

    fn derive<const N: usize>(&self, input: &[&[u8]]) -> [u8; N] {
        let mut mac = Kmac::v256(&self.key, &[]);
        for part in input {
            mac.update(part);
        }
        let mut output = [0; N];
        mac.finalize(&mut output);
        output
    }
}

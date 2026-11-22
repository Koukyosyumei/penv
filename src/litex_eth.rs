use std::collections::VecDeque;

const SLOT_SIZE: usize = 0x800;
const SLOTS: usize = 2;
const MIN_FRAME_SIZE: usize = 18;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Region {
    Mac,
    Buffer,
    Phy,
}

pub struct Registration {
    pub base: u32,
    pub size: u32,
    pub region: Region,
}

pub struct Phy {
    pub address: u8,
    pub registers: [u16; 32],
}

impl Phy {
    pub fn new(address: u8) -> Self {
        let mut registers = [0; 32];
        registers[1] = 0x24; // link up, autonegotiation complete
        Self { address, registers }
    }
}

#[derive(Default)]
enum MdioState {
    #[default]
    Idle,
    Preamble,
    Command,
    WriteData,
}

#[derive(Default)]
struct Mdio {
    state: MdioState,
    clock: bool,
    input: u16,
    count: u32,
    output: u16,
    output_position: Option<u32>,
}

impl Mdio {
    fn write(&mut self, value: u32, phy: &Phy) {
        let clock = value & 1 != 0;
        let rising = clock && !self.clock;
        self.clock = clock;
        if !rising {
            return;
        }
        self.output_position = self
            .output_position
            .and_then(|position| position.checked_sub(1));
        self.input |= (((value >> 2) & 1) as u16) << self.count;
        self.count += 1;
        if self.count != 16 {
            return;
        }
        self.count = 0;
        let word = std::mem::take(&mut self.input);
        self.state = match self.state {
            MdioState::Idle if word == 0xffff => MdioState::Preamble,
            MdioState::Preamble if word == 0xffff => MdioState::Command,
            MdioState::Command if word & 3 == 2 => {
                let operation = (word >> 2) & 3;
                let address = ((word >> 4) as u8).reverse_bits() >> 3;
                let register = ((word >> 9) as u8).reverse_bits() >> 3;
                match operation {
                    1 => {
                        self.output = if address == phy.address {
                            phy.registers[usize::from(register)]
                        } else {
                            0
                        };
                        self.output_position = Some(16);
                        MdioState::Idle
                    }
                    2 => MdioState::WriteData,
                    _ => MdioState::Idle,
                }
            }
            _ => MdioState::Idle,
        };
    }

    fn read(&self) -> u32 {
        self.output_position
            .filter(|&position| position < 16)
            .map_or(0, |position| u32::from((self.output >> position) & 1))
    }
}

pub struct Ethernet {
    pub registrations: Vec<Registration>,
    pub name: Option<String>,
    pub tftp: Option<crate::litex_tftp::Server>,
    pub vex_irq: Option<(usize, u8)>,
    buffer: Box<[u8]>,
    tx_lengths: [u32; SLOTS],
    wide_csr: bool,
    tx_slot: usize,
    tx_pending: bool,
    tx_enabled: bool,
    rx_enabled: bool,
    rx_lengths: [usize; SLOTS],
    rx_order: VecDeque<usize>,
    rx_latched: Option<usize>,
    phy: Option<Phy>,
    mdio: Mdio,
}

impl Ethernet {
    pub const MAC_SIZE: u32 = 0x100;

    pub fn new(
        registrations: Vec<Registration>,
        vex_irq: Option<(usize, u8)>,
        phy: Option<Phy>,
    ) -> Self {
        Self {
            registrations,
            name: None,
            tftp: None,
            vex_irq,
            buffer: vec![0; 2 * SLOTS * SLOT_SIZE].into_boxed_slice(),
            tx_lengths: [0; SLOTS],
            wide_csr: false,
            tx_slot: 0,
            tx_pending: false,
            tx_enabled: false,
            rx_enabled: false,
            rx_lengths: [0; SLOTS],
            rx_order: VecDeque::new(),
            rx_latched: None,
            phy,
            mdio: Mdio::default(),
        }
    }

    pub fn with_wide_csr(mut self, wide_csr: bool) -> Self {
        self.wide_csr = wide_csr;
        self
    }

    fn mac_register(&self, offset: u32) -> u32 {
        if !self.wide_csr {
            return offset & !3;
        }
        match offset & !3 {
            0x00 => 0x00,
            0x04 => 0x04,
            0x10 => 0x28,
            0x14 => 0x2c,
            0x18 => 0x30,
            0x1c => 0x34,
            0x24 => 0x3c,
            0x28 => 0x40,
            0x30 => 0x4c,
            0x34 => 0x50,
            _ => u32::MAX,
        }
    }

    pub fn interrupt_pending(&self) -> bool {
        self.tx_pending && self.tx_enabled || self.rx_enabled && !self.rx_order.is_empty()
    }

    pub fn locate(&self, address: u32, width: usize) -> Option<(Region, u32)> {
        self.registrations.iter().find_map(|registration| {
            let offset = address.checked_sub(registration.base)?;
            (width <= registration.size.checked_sub(offset)? as usize)
                .then_some((registration.region, offset))
        })
    }

    pub fn read(&mut self, region: Region, offset: u32, width: usize) -> u32 {
        match region {
            Region::Mac => {
                let length = self
                    .rx_order
                    .front()
                    .map_or(0, |&slot| self.rx_lengths[slot]) as u32;
                let value = match self.mac_register(offset) {
                    0 => {
                        self.rx_latched = self.rx_order.front().copied();
                        self.rx_latched.unwrap_or(0) as u32
                    }
                    4 if self.wide_csr => length,
                    4..=0x10 => (length >> ((0x10 - (offset & !3)) * 2)) & 0xff,
                    0x28 => u32::from(!self.rx_order.is_empty()),
                    0x2c => u32::from(self.rx_enabled),
                    0x34 => 1,
                    0x3c => self.tx_slot as u32,
                    0x40 if self.wide_csr => self.tx_lengths[self.tx_slot],
                    0x40 => self.tx_lengths[self.tx_slot] >> 8,
                    0x44 => self.tx_lengths[self.tx_slot] & 0xff,
                    0x4c => u32::from(self.tx_pending),
                    0x50 => u32::from(self.tx_enabled),
                    _ => 0,
                };
                (value >> ((offset & 3) * 8)) & lane_mask(width)
            }
            Region::Buffer => {
                let Some(bytes) = self.buffer.get(offset as usize..offset as usize + width) else {
                    return 0;
                };
                let mut value = [0; 4];
                value[..width].copy_from_slice(bytes);
                u32::from_le_bytes(value)
            }
            Region::Phy if offset == 8 => self.mdio.read(),
            Region::Phy => 0,
        }
    }

    pub fn write(&mut self, region: Region, offset: u32, width: usize, value: u32) {
        match region {
            Region::Mac => {
                let register = self.mac_register(offset);
                if register == 0x40 && self.wide_csr {
                    let shift = (offset & 3) * 8;
                    let mask = lane_mask(width) << shift;
                    let length = &mut self.tx_lengths[self.tx_slot];
                    *length = (*length & !mask) | ((value << shift) & mask);
                    return;
                }
                if offset & 3 != 0 {
                    return;
                }
                let enable = value & 1 != 0;
                match register {
                    0x28 if enable => {
                        if let Some(slot) = self.rx_latched.take() {
                            self.rx_lengths[slot] = 0;
                            self.rx_order.retain(|&pending| pending != slot);
                        } else {
                            self.rx_lengths.fill(0);
                            self.rx_order.clear();
                        }
                    }
                    0x2c => self.rx_enabled = enable,
                    0x30 => self.transmit(),
                    0x3c => {
                        let slot = (value & lane_mask(width)) as usize;
                        self.tx_slot = if slot < SLOTS { slot } else { 0 };
                    }
                    0x40 => {
                        let length = &mut self.tx_lengths[self.tx_slot];
                        *length = (*length & 0xff) | (u32::from(value as u8) << 8);
                    }
                    0x44 => {
                        let length = &mut self.tx_lengths[self.tx_slot];
                        *length = (*length & 0xff00) | u32::from(value as u8);
                    }
                    0x4c if enable => self.tx_pending = false,
                    0x50 => self.tx_enabled = enable,
                    _ => {}
                }
            }
            Region::Buffer => {
                if let Some(bytes) = self
                    .buffer
                    .get_mut(offset as usize..offset as usize + width)
                {
                    bytes.copy_from_slice(&value.to_le_bytes()[..width]);
                }
            }
            Region::Phy if offset == 4 => {
                if let Some(phy) = self.phy.as_ref() {
                    self.mdio.write(value, phy);
                }
            }
            _ => {}
        }
    }

    fn transmit(&mut self) {
        let length = self.tx_lengths[self.tx_slot] as usize;
        if !(MIN_FRAME_SIZE..=SLOT_SIZE).contains(&length) {
            return;
        }
        self.tx_pending = true;
        let start = (SLOTS + self.tx_slot) * SLOT_SIZE;
        if let Some(frame) = self
            .tftp
            .as_mut()
            .and_then(|server| server.reply(&self.buffer[start..start + length]))
        {
            self.receive(&frame);
        }
    }

    fn receive(&mut self, frame: &[u8]) {
        if frame.is_empty() || frame.len() > SLOT_SIZE {
            return;
        }
        let Some(slot) = self.rx_lengths.iter().position(|&length| length == 0) else {
            return;
        };
        let start = slot * SLOT_SIZE;
        self.buffer[start..start + frame.len()].copy_from_slice(frame);
        self.rx_lengths[slot] = frame.len();
        self.rx_order.push_back(slot);
    }
}

fn lane_mask(width: usize) -> u32 {
    u32::MAX >> ((4 - width) * 8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_configured_phy_over_mdio() {
        let mut phy = Phy::new(0);
        phy.registers[17] = 0x4400;
        let mut eth = Ethernet::new(Vec::new(), None, Some(phy));
        let mut clock = |bit: u32| {
            eth.write(Region::Phy, 4, 4, bit << 2);
            eth.write(Region::Phy, 4, 4, (bit << 2) | 1);
            eth.read(Region::Phy, 8, 4)
        };
        for _ in 0..32 {
            clock(1);
        }
        let command = (0b10001u16 << 9) | (1 << 2) | 2;
        for shift in 0..16 {
            clock(u32::from((command >> shift) & 1));
        }
        let mut value = 0;
        for _ in 0..16 {
            value = (value << 1) | clock(0);
        }
        assert_eq!(value, 0x4400);
    }

    #[test]
    fn shares_buffer_storage_and_signals_transmit_completion() {
        let regs = vec![
            Registration {
                base: 0x3000_0000,
                size: 0x2000,
                region: Region::Buffer,
            },
            Registration {
                base: 0xb000_0000,
                size: 0x2000,
                region: Region::Buffer,
            },
        ];
        let mut eth = Ethernet::new(regs, Some((0, 3)), None);
        let (region, offset) = eth.locate(0x3000_1000, 4).unwrap();
        eth.write(region, offset, 4, 0x1234_5678);
        let (region, offset) = eth.locate(0xb000_1001, 1).unwrap();
        assert_eq!(eth.read(region, offset, 1), 0x56);
        assert!(eth.locate(0xb000_1fff, 4).is_none());
        eth.write(Region::Mac, 0x44, 1, 64);
        eth.write(Region::Mac, 0x30, 4, 1);
        assert!(!eth.interrupt_pending());
        eth.write(Region::Mac, 0x50, 4, 1);
        assert!(eth.interrupt_pending());
        eth.write(Region::Mac, 0x4c, 4, 1);
        assert!(!eth.interrupt_pending());
    }
}

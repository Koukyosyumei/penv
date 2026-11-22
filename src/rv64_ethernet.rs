// Synopsys DesignWare MAC register and Clause 22 management interface used
// by the BeagleV firmware. Packet DMA is outside this register model.
const MAC_CONFIG: u32 = 0x00;
const FRAME_FILTER: u32 = 0x04;
const MII_ADDRESS: u32 = 0x10;
const MII_DATA: u32 = 0x14;
const ADDRESS_HIGH: u32 = 0x40;
const ADDRESS_LOW: u32 = 0x44;
const DMA_BUS_MODE: u32 = 0x1000;
const DMA_RX_LIST: u32 = 0x100c;
const DMA_TX_LIST: u32 = 0x1010;
const DMA_OPERATION: u32 = 0x1018;
const DMA_INTERRUPT_ENABLE: u32 = 0x101c;
const DMA_CURRENT_TX: u32 = 0x1048;
const DMA_CURRENT_RX: u32 = 0x104c;

pub struct Ethernet {
    pub base: u32,
    phys: [Option<Phy>; 32],
    registers: Registers,
}

#[derive(Clone, Copy)]
pub struct Phy {
    pub registers: [u16; 32],
}

impl Default for Phy {
    fn default() -> Self {
        let mut registers = [0; 32];
        registers[1] = 0x24; // Link up, autonegotiation complete.
        Self { registers }
    }
}

struct Registers {
    config: u32,
    filter: u32,
    mii_address: u32,
    mii_data: u16,
    mac_high: u32,
    mac_low: u32,
    bus_mode: u32,
    receive_list: u32,
    transmit_list: u32,
    operation: u32,
    interrupt_enable: u32,
}

impl Default for Registers {
    fn default() -> Self {
        Self {
            config: 0x8000,
            filter: 0,
            mii_address: 0,
            mii_data: 0,
            mac_high: 0x8000_ffff,
            mac_low: u32::MAX,
            bus_mode: 0x0002_0100,
            receive_list: 0,
            transmit_list: 0,
            operation: 0,
            interrupt_enable: 0,
        }
    }
}

impl Ethernet {
    pub const SIZE: u64 = 0x1400;

    pub fn new(base: u32) -> Self {
        Self {
            base,
            phys: [None; 32],
            registers: Registers::default(),
        }
    }

    pub fn attach(&mut self, address: usize, phy: Phy) -> bool {
        let Some(slot) = self.phys.get_mut(address) else {
            return false;
        };
        if slot.is_some() {
            return false;
        }
        *slot = Some(phy);
        true
    }

    pub fn contains(&self, address: u32) -> bool {
        u64::from(address.wrapping_sub(self.base)) < Self::SIZE
    }

    pub fn read(&self, address: u32) -> u32 {
        let registers = &self.registers;
        match address - self.base {
            MAC_CONFIG => registers.config,
            FRAME_FILTER => registers.filter,
            MII_ADDRESS => registers.mii_address,
            MII_DATA => u32::from(registers.mii_data),
            ADDRESS_HIGH => registers.mac_high,
            ADDRESS_LOW => registers.mac_low,
            DMA_BUS_MODE => registers.bus_mode,
            DMA_RX_LIST | DMA_CURRENT_RX => registers.receive_list,
            DMA_TX_LIST | DMA_CURRENT_TX => registers.transmit_list,
            DMA_OPERATION => registers.operation,
            DMA_INTERRUPT_ENABLE => registers.interrupt_enable,
            _ => 0,
        }
    }

    pub fn write(&mut self, address: u32, value: u32) {
        let registers = &mut self.registers;
        match address - self.base {
            MAC_CONFIG => registers.config = value & 0x02cf_7efc,
            FRAME_FILTER => registers.filter = value & 0x8000_07ff,
            MII_ADDRESS => {
                registers.mii_address = value & 0xffde;
                let phy = ((value >> 11) & 31) as usize;
                let register = ((value >> 6) & 31) as usize;
                // Renode's generic PHY returns REPL values and ignores writes.
                // Transactions complete immediately, even at vacant PHY addresses.
                if value & 2 == 0
                    && let Some(phy) = self.phys[phy]
                {
                    registers.mii_data = phy.registers[register];
                }
            }
            MII_DATA => registers.mii_data = value as u16,
            ADDRESS_HIGH => registers.mac_high = 0x8000_0000 | (value & 0xffff),
            ADDRESS_LOW => registers.mac_low = value,
            DMA_BUS_MODE if value & 1 != 0 => self.registers = Registers::default(),
            DMA_BUS_MODE => registers.bus_mode = value & 0x07ff_fffe,
            DMA_RX_LIST | DMA_CURRENT_RX => registers.receive_list = value & !3,
            DMA_TX_LIST | DMA_CURRENT_TX => registers.transmit_list = value & !3,
            DMA_OPERATION => registers.operation = value & 0x0721_e0de,
            DMA_INTERRUPT_ENABLE => registers.interrupt_enable = value & 0x1_e7ff,
            _ => {}
        }
    }
}

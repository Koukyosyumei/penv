use crate::bus::Bus;

// Synopsys DesignWare MAC management and transmit DMA for BeagleV.
// There is no attached packet transport; transmitted frames leave the guest
// without a receiver. RX descriptors remain untouched.
const MAC_CONFIG: u32 = 0x00;
const FRAME_FILTER: u32 = 0x04;
const MII_ADDRESS: u32 = 0x10;
const MII_DATA: u32 = 0x14;
const ADDRESS_HIGH: u32 = 0x40;
const ADDRESS_LOW: u32 = 0x44;
const DMA_BUS_MODE: u32 = 0x1000;
const DMA_TX_POLL: u32 = 0x1004;
const DMA_RX_LIST: u32 = 0x100c;
const DMA_TX_LIST: u32 = 0x1010;
const DMA_STATUS: u32 = 0x1014;
const DMA_OPERATION: u32 = 0x1018;
const DMA_INTERRUPT_ENABLE: u32 = 0x101c;
const DMA_CURRENT_TX: u32 = 0x1048;
const DMA_CURRENT_RX: u32 = 0x104c;

const MAC_TX_ENABLE: u32 = 1 << 3;
const DMA_START_TX: u32 = 1 << 13;
const TX_COMPLETE: u32 = 1;
const TX_UNAVAILABLE: u32 = 1 << 2;
const FATAL_BUS_ERROR: u32 = 1 << 13;
const ABNORMAL_SUMMARY: u32 = 1 << 15;
const NORMAL_SUMMARY: u32 = 1 << 16;
const DESCRIPTOR_OWN: u32 = 1 << 31;
const INTERRUPT_ON_COMPLETION: u32 = 1 << 31;
const LAST_SEGMENT: u32 = 1 << 30;
const END_OF_RING: u32 = 1 << 25;
const CHAINED: u32 = 1 << 24;

pub struct Ethernet {
    pub base: u32,
    pub irq: usize,
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
    current_tx: u32,
    status: u32,
    tx_state: u32,
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
            current_tx: 0,
            status: 0,
            tx_state: 0,
            operation: 0,
            interrupt_enable: 0,
        }
    }
}

impl Ethernet {
    pub const SIZE: u64 = 0x1400;

    pub fn new(base: u32, irq: usize) -> Self {
        Self {
            base,
            irq,
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
            DMA_TX_LIST => registers.transmit_list,
            DMA_CURRENT_TX => registers.current_tx,
            DMA_STATUS => registers.status | (registers.tx_state << 20),
            DMA_OPERATION => registers.operation,
            DMA_INTERRUPT_ENABLE => registers.interrupt_enable,
            _ => 0,
        }
    }

    fn write(&mut self, address: u32, value: u32, bus: &mut Bus) {
        let registers = &mut self.registers;
        let offset = address - self.base;
        match offset {
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
            DMA_BUS_MODE if value & 1 != 0 => *registers = Registers::default(),
            DMA_BUS_MODE => registers.bus_mode = value & 0x07ff_fffe,
            DMA_RX_LIST | DMA_CURRENT_RX => registers.receive_list = value & !3,
            DMA_TX_LIST => {
                registers.transmit_list = value & !3;
                registers.current_tx = registers.transmit_list;
            }
            DMA_STATUS => registers.status &= !(value & 0x0001_e7ff),
            DMA_CURRENT_TX => registers.current_tx = value & !3,
            DMA_OPERATION => registers.operation = value & 0x0721_e0de,
            DMA_INTERRUPT_ENABLE => registers.interrupt_enable = value & 0x1_e7ff,
            _ => {}
        }
        if registers.operation & DMA_START_TX == 0 {
            registers.tx_state = 0;
        } else if registers.config & MAC_TX_ENABLE != 0
            && matches!(offset, MAC_CONFIG | DMA_TX_POLL | DMA_OPERATION)
        {
            self.transmit(bus);
        }
        self.update_interrupt_summary();
    }

    fn update_interrupt_summary(&mut self) {
        let active = self.registers.status & self.registers.interrupt_enable;
        if active & (TX_COMPLETE | TX_UNAVAILABLE) != 0 {
            self.registers.status |= NORMAL_SUMMARY;
        }
        if active & FATAL_BUS_ERROR != 0 {
            self.registers.status |= ABNORMAL_SUMMARY;
        }
    }

    fn interrupt_pending(&self) -> bool {
        self.registers.status
            & self.registers.interrupt_enable
            & (NORMAL_SUMMARY | ABNORMAL_SUMMARY)
            != 0
    }

    fn transmit(&mut self, bus: &mut Bus) {
        let registers = &mut self.registers;
        // Limit processing per poll demand, even for a malformed descriptor ring.
        for _ in 0..4096 {
            let address = u64::from(registers.current_tx);
            let Some(bytes) = bus.ram64(address, 16) else {
                registers.tx_fault();
                break;
            };
            let descriptor =
                |index: usize| u32::from_le_bytes(bytes[index..index + 4].try_into().unwrap());
            let status = descriptor(0);
            if status & DESCRIPTOR_OWN == 0 {
                registers.tx_state = 6; // Suspended until the next poll demand.
                registers.status |= TX_UNAVAILABLE;
                break;
            }
            let control = descriptor(4);
            let buffer = descriptor(8);
            let second = descriptor(12);
            let first_size = (control & 0x3ff) as usize;
            let second_size = ((control >> 11) & 0x3ff) as usize;
            let chained = control & CHAINED != 0;
            if (first_size != 0 && bus.ram64(u64::from(buffer), first_size).is_none())
                || (!chained
                    && second_size != 0
                    && bus.ram64(u64::from(second), second_size).is_none())
            {
                registers.tx_fault();
                break;
            }

            let next = if chained {
                second & !3
            } else if control & END_OF_RING != 0 {
                registers.transmit_list
            } else {
                registers
                    .current_tx
                    .wrapping_add(16 + ((registers.bus_mode >> 2) & 31) * 4)
            };
            bus.memory64(address, 4)
                .expect("descriptor was read from RAM")
                .copy_from_slice(&(status & !DESCRIPTOR_OWN).to_le_bytes());
            registers.current_tx = next;
            registers.tx_state = 7; // Closing descriptor.
            if control & (LAST_SEGMENT | INTERRUPT_ON_COMPLETION)
                == LAST_SEGMENT | INTERRUPT_ON_COMPLETION
            {
                registers.status |= TX_COMPLETE;
            }
        }
    }
}

impl Registers {
    fn tx_fault(&mut self) {
        self.tx_state = 0;
        self.operation &= !DMA_START_TX;
        self.status |= FATAL_BUS_ERROR;
    }
}

impl Bus {
    pub fn write_rv64_ethernet(&mut self, address: u32, width: usize, value: u32) -> bool {
        if width != 4 {
            return false;
        }
        let Some(index) = self
            .rv64_devices
            .ethernet
            .iter()
            .position(|device| device.contains(address))
        else {
            return false;
        };
        // Detach the MACs while running DMA so that guest RAM can be borrowed.
        let mut macs = std::mem::take(&mut self.rv64_devices.ethernet);
        let device = &mut macs[index];
        device.write(address, value, self);
        self.rv64_devices
            .set_ethernet_irq(device.irq, device.interrupt_pending());
        self.rv64_devices.ethernet = macs;
        true
    }
}

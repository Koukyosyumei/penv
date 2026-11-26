use std::{
    collections::VecDeque,
    net::{Ipv4Addr, UdpSocket},
    sync::{Arc, Mutex},
};

use crate::bus::Bus;

// Synopsys DesignWare MAC and normal DMA descriptors for BeagleV.
const MAC_CONFIG: u32 = 0x00;
const FRAME_FILTER: u32 = 0x04;
const MII_ADDRESS: u32 = 0x10;
const MII_DATA: u32 = 0x14;
const ADDRESS_HIGH: u32 = 0x40;
const ADDRESS_LOW: u32 = 0x44;
const DMA_BUS_MODE: u32 = 0x1000;
const DMA_TX_POLL: u32 = 0x1004;
const DMA_RX_POLL: u32 = 0x1008;
const DMA_RX_LIST: u32 = 0x100c;
const DMA_TX_LIST: u32 = 0x1010;
const DMA_STATUS: u32 = 0x1014;
const DMA_OPERATION: u32 = 0x1018;
const DMA_INTERRUPT_ENABLE: u32 = 0x101c;
const DMA_CURRENT_TX: u32 = 0x1048;
const DMA_CURRENT_RX: u32 = 0x104c;

const MAC_TX_ENABLE: u32 = 1 << 3;
const MAC_RX_ENABLE: u32 = 1 << 2;
const MAC_STRIP_FCS: u32 = 1 << 25;
const DMA_START_TX: u32 = 1 << 13;
const DMA_START_RX: u32 = 1 << 1;
const TX_COMPLETE: u32 = 1;
const TX_UNAVAILABLE: u32 = 1 << 2;
const RX_COMPLETE: u32 = 1 << 6;
const RX_UNAVAILABLE: u32 = 1 << 7;
const FATAL_BUS_ERROR: u32 = 1 << 13;
const ABNORMAL_SUMMARY: u32 = 1 << 15;
const NORMAL_SUMMARY: u32 = 1 << 16;
const DESCRIPTOR_OWN: u32 = 1 << 31;
const INTERRUPT_ON_COMPLETION: u32 = 1 << 31;
const LAST_SEGMENT: u32 = 1 << 30;
const FIRST_SEGMENT: u32 = 1 << 29;
const END_OF_RING: u32 = 1 << 25;
const CHAINED: u32 = 1 << 24;
const RX_FIRST: u32 = 1 << 9;
const RX_LAST: u32 = 1 << 8;
const RX_DISABLE_INTERRUPT: u32 = 1 << 31;
const RX_DESCRIPTOR_ERROR: u32 = (1 << 15) | (1 << 14);
const MAX_FRAME: usize = 16_383;
const MAX_DESCRIPTORS_PER_POLL: usize = 4096;

#[derive(Default)]
pub(crate) struct LocalSwitch {
    ports: Arc<Mutex<Vec<Option<VecDeque<Vec<u8>>>>>>,
}

struct LocalPort {
    ports: Arc<Mutex<Vec<Option<VecDeque<Vec<u8>>>>>>,
    index: usize,
}

impl LocalSwitch {
    fn connect(&self) -> LocalPort {
        let mut ports = self.ports.lock().expect("Ethernet switch lock");
        let index = ports.iter().position(Option::is_none).unwrap_or(ports.len());
        if index == ports.len() {
            ports.push(Some(VecDeque::new()));
        } else {
            ports[index] = Some(VecDeque::new());
        }
        LocalPort { ports: self.ports.clone(), index }
    }
}

impl LocalPort {
    fn send(&self, frame: &[u8]) {
        let mut ports = self.ports.lock().expect("Ethernet switch lock");
        for (index, queue) in ports.iter_mut().enumerate() {
            if index != self.index && let Some(queue) = queue && queue.len() < 128 {
                queue.push_back(frame.to_vec());
            }
        }
    }

    fn recv(&self, buffer: &mut [u8]) -> Option<usize> {
        let frame = self.ports.lock().expect("Ethernet switch lock")[self.index].as_mut()?.pop_front()?;
        buffer[..frame.len()].copy_from_slice(&frame);
        Some(frame.len())
    }
}

impl Drop for LocalPort {
    fn drop(&mut self) {
        self.ports.lock().expect("Ethernet switch lock")[self.index] = None;
    }
}

pub struct Ethernet {
    pub base: u32,
    pub irq: usize,
    phys: [Option<Phy>; 32],
    registers: Registers,
    link: Option<UdpSocket>,
    local: Option<LocalPort>,
    tx_frame: Vec<u8>,
    tx_dropping: bool,
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
    current_rx: u32,
    status: u32,
    tx_state: u32,
    rx_state: u32,
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
            current_rx: 0,
            status: 0,
            tx_state: 0,
            rx_state: 0,
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
            link: None,
            local: None,
            tx_frame: Vec::new(),
            tx_dropping: false,
        }
    }

    pub(crate) fn supports_snapshot(&self) -> bool {
        !self.has_link()
    }

    pub(crate) fn save_snapshot(&self, output: &mut impl std::io::Write) -> std::io::Result<()> {
        use crate::snapshot::{invalid, write_words};
        if !self.supports_snapshot() {
            return Err(invalid("cannot snapshot Ethernet with an active host link"));
        }
        write_words(output, &[
            u64::from(self.base), self.irq as u64, u64::from(self.tx_dropping), self.tx_frame.len() as u64,
        ])?;
        let registers = &self.registers;
        write_words(output, &[
            u64::from(registers.config),
            u64::from(registers.filter),
            u64::from(registers.mii_address),
            u64::from(registers.mii_data),
            u64::from(registers.mac_high),
            u64::from(registers.mac_low),
            u64::from(registers.bus_mode),
            u64::from(registers.receive_list),
            u64::from(registers.transmit_list),
            u64::from(registers.current_tx),
            u64::from(registers.current_rx),
            u64::from(registers.status),
            u64::from(registers.tx_state),
            u64::from(registers.rx_state),
            u64::from(registers.operation),
            u64::from(registers.interrupt_enable),
        ])?;
        for phy in &self.phys {
            write_words(output, &[u64::from(phy.is_some())])?;
            if let Some(phy) = phy {
                write_words(output, &phy.registers.map(u64::from))?;
            }
        }
        output.write_all(&self.tx_frame)
    }

    pub(crate) fn read_snapshot(&self, input: &mut impl std::io::Read) -> std::io::Result<Self> {
        use crate::snapshot::{invalid, read_bool, read_words};
        let [base, irq, dropping, length] = read_words(input)?;
        if base != u64::from(self.base) || irq != self.irq as u64 || length > (MAX_FRAME - 4) as u64 {
            return Err(invalid("invalid Ethernet snapshot layout or frame length"));
        }
        let [
            config,
            filter,
            mii_address,
            mii_data,
            mac_high,
            mac_low,
            bus_mode,
            receive_list,
            transmit_list,
            current_tx,
            current_rx,
            status,
            tx_state,
            rx_state,
            operation,
            interrupt_enable,
        ] = read_words(input)?;
        let word = |value| u32::try_from(value).map_err(std::io::Error::other);
        let registers = Registers {
            config: word(config)?,
            filter: word(filter)?,
            mii_address: word(mii_address)?,
            mii_data: u16::try_from(mii_data).map_err(std::io::Error::other)?,
            mac_high: word(mac_high)?,
            mac_low: word(mac_low)?,
            bus_mode: word(bus_mode)?,
            receive_list: word(receive_list)?,
            transmit_list: word(transmit_list)?,
            current_tx: word(current_tx)?,
            current_rx: word(current_rx)?,
            status: word(status)?,
            tx_state: word(tx_state)?,
            rx_state: word(rx_state)?,
            operation: word(operation)?,
            interrupt_enable: word(interrupt_enable)?,
        };
        let mut phys = [None; 32];
        for (index, slot) in phys.iter_mut().enumerate() {
            if read_bool(read_words::<1>(input)?[0])? != self.phys[index].is_some() {
                return Err(invalid("Ethernet snapshot PHY layout differs from the platform"));
            }
            if self.phys[index].is_some() {
                let saved: [u64; 32] = read_words(input)?;
                let mut phy = Phy::default();
                for (register, word) in phy.registers.iter_mut().zip(saved) {
                    *register = u16::try_from(word).map_err(std::io::Error::other)?;
                }
                *slot = Some(phy);
            }
        }
        let mut tx_frame = vec![0; length as usize];
        input.read_exact(&mut tx_frame)?;
        Ok(Self { base: self.base, irq: self.irq, registers, phys, link: None, local: None, tx_frame,
            tx_dropping: read_bool(dropping)? })
    }

    fn has_link(&self) -> bool {
        self.link.is_some() || self.local.is_some()
    }

    fn recv_frame(&self, buffer: &mut [u8]) -> Option<usize> {
        if let Some(local) = &self.local {
            local.recv(buffer)
        } else {
            self.link.as_ref()?.recv(buffer).ok()
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
            DMA_RX_LIST => registers.receive_list,
            DMA_CURRENT_RX => registers.current_rx,
            DMA_TX_LIST => registers.transmit_list,
            DMA_CURRENT_TX => registers.current_tx,
            DMA_STATUS => {
                registers.status | (registers.tx_state << 20) | (registers.rx_state << 17)
            }
            DMA_OPERATION => registers.operation,
            DMA_INTERRUPT_ENABLE => registers.interrupt_enable,
            _ => 0,
        }
    }

    fn write(&mut self, address: u32, value: u32, bus: &mut Bus) {
        let offset = address - self.base;
        if offset == DMA_BUS_MODE && value & 1 != 0 {
            self.registers = Registers::default();
            self.tx_frame.clear();
            self.tx_dropping = false;
            // The MAC's receive FIFO is reset along with its descriptor state.
            // Discard queued link frames while the receiver is disabled.
            if self.has_link() {
                let mut frame = [0; MAX_FRAME + 1];
                for _ in 0..4096 {
                    if self.recv_frame(&mut frame).is_none() {
                        break;
                    }
                }
            }
            return;
        }
        let registers = &mut self.registers;
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
            DMA_BUS_MODE => registers.bus_mode = value & 0x07ff_fffe,
            DMA_RX_LIST => {
                registers.receive_list = value & !3;
                registers.current_rx = registers.receive_list;
            }
            DMA_CURRENT_RX => registers.current_rx = value & !3,
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
        let poll_rx = registers.operation & DMA_START_RX != 0
            && matches!(offset, MAC_CONFIG | DMA_RX_POLL | DMA_OPERATION);
        if registers.operation & DMA_START_RX == 0 {
            registers.rx_state = 0;
        } else if poll_rx {
            registers.rx_state = 3; // Waiting for a frame.
        }
        if registers.operation & DMA_START_TX == 0 {
            registers.tx_state = 0;
        } else if registers.config & MAC_TX_ENABLE != 0
            && matches!(offset, MAC_CONFIG | DMA_TX_POLL | DMA_OPERATION)
        {
            self.transmit(bus);
        }
        if poll_rx {
            self.poll_link(bus);
        }
        self.update_interrupt_summary();
    }

    fn update_interrupt_summary(&mut self) {
        let active = self.registers.status & self.registers.interrupt_enable;
        if active & (TX_COMPLETE | TX_UNAVAILABLE | RX_COMPLETE) != 0 {
            self.registers.status |= NORMAL_SUMMARY;
        }
        if active & (FATAL_BUS_ERROR | RX_UNAVAILABLE) != 0 {
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
        for _ in 0..MAX_DESCRIPTORS_PER_POLL {
            let address = u64::from(registers.current_tx);
            let Some([status, control, buffer, second]) = descriptor(bus, address) else {
                registers.tx_fault();
                self.tx_frame.clear();
                self.tx_dropping = false;
                break;
            };
            if status & DESCRIPTOR_OWN == 0 {
                registers.tx_state = 6; // Suspended until the next poll demand.
                registers.status |= TX_UNAVAILABLE;
                break;
            }
            let first_size = (control & 0x3ff) as usize;
            let second_size = ((control >> 11) & 0x3ff) as usize;
            let chained = control & CHAINED != 0;
            let buffers = [
                (buffer, first_size),
                (second, if chained { 0 } else { second_size }),
            ];
            if control & FIRST_SEGMENT != 0 {
                self.tx_frame.clear();
                self.tx_dropping = false;
            }
            let mut unmapped = false;
            for (buffer, size) in buffers.into_iter().filter(|&(_, size)| size != 0) {
                let Some(bytes) = bus.ram64(u64::from(buffer), size) else {
                    unmapped = true;
                    break;
                };
                if (self.link.is_some() || self.local.is_some()) && !self.tx_dropping {
                    if self.tx_frame.len() + size <= MAX_FRAME - 4 {
                        self.tx_frame.extend_from_slice(bytes);
                    } else {
                        self.tx_dropping = true;
                    }
                }
            }
            if unmapped {
                registers.tx_fault();
                self.tx_frame.clear();
                self.tx_dropping = false;
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
            write_descriptor_status(bus, address, status & !DESCRIPTOR_OWN);
            registers.current_tx = next;
            registers.tx_state = 7; // Closing descriptor.
            if control & LAST_SEGMENT != 0 {
                if (self.link.is_some() || self.local.is_some())
                    && !self.tx_dropping
                    && self.tx_frame.len() >= 14
                {
                    // The loopback transport carries frames without the wire FCS.
                    self.tx_frame.resize(self.tx_frame.len().max(60), 0);
                    if let Some(local) = &self.local {
                        local.send(&self.tx_frame);
                    } else if let Some(link) = &self.link {
                        let _ = link.send(&self.tx_frame);
                    }
                }
                self.tx_frame.clear();
                self.tx_dropping = false;
                if control & INTERRUPT_ON_COMPLETION != 0 {
                    registers.status |= TX_COMPLETE;
                }
            }
        }
    }

    fn poll_link(&mut self, bus: &mut Bus) {
        if !self.has_link() {
            return;
        }
        let mut bytes = [0; MAX_FRAME + 1];
        // Bound host input per poll, including when the guest receiver is disabled.
        for _ in 0..32 {
            let Some(length) = self.recv_frame(&mut bytes) else {
                break;
            };
            if self.registers.config & MAC_RX_ENABLE == 0
                || self.registers.operation & DMA_START_RX == 0
                || !(14..=MAX_FRAME - 4).contains(&length)
                || !self.accepts(&bytes[..6])
            {
                continue;
            }
            let frame_length = if self.registers.config & MAC_STRIP_FCS == 0 {
                let fcs = ethernet_crc(&bytes[..length]);
                bytes[length..length + 4].copy_from_slice(&fcs.to_le_bytes());
                length + 4
            } else {
                length
            };
            self.registers.receive(bus, &bytes[..frame_length]);
        }
        self.update_interrupt_summary();
    }

    fn accepts(&self, destination: &[u8]) -> bool {
        let registers = &self.registers;
        let filter = registers.filter;
        if filter & (1 | (1 << 31)) != 0 {
            return true;
        }
        if destination == [0xff; 6] {
            return filter & (1 << 5) == 0;
        }
        if destination[0] & 1 != 0 {
            return filter & ((1 << 2) | (1 << 4)) != 0;
        }
        destination[..4] == registers.mac_low.to_le_bytes()
            && destination[4..] == (registers.mac_high as u16).to_le_bytes()
    }
}

impl Registers {
    fn receive(&mut self, bus: &mut Bus, frame: &[u8]) {
        let mut copied = 0;
        let mut first = true;
        for processed in 1..=MAX_DESCRIPTORS_PER_POLL {
            let address = u64::from(self.current_rx);
            let Some([status, control, first_buffer, second_buffer]) = descriptor(bus, address)
            else {
                self.rx_fault();
                return;
            };
            if status & DESCRIPTOR_OWN == 0 {
                self.rx_unavailable();
                return;
            }
            let chained = control & CHAINED != 0;
            let buffers = [
                (first_buffer, (control & 0x7ff) as usize),
                (
                    second_buffer,
                    if chained {
                        0
                    } else {
                        ((control >> 11) & 0x7ff) as usize
                    },
                ),
            ];
            let mut result = if first { RX_FIRST } else { 0 };
            first = false;
            for (buffer, size) in buffers {
                let count = size.min(frame.len() - copied);
                if count == 0 {
                    continue;
                }
                let Some(bytes) = bus.memory64(u64::from(buffer), count) else {
                    write_descriptor_status(bus, address, result | RX_LAST | RX_DESCRIPTOR_ERROR);
                    self.rx_fault();
                    return;
                };
                bytes.copy_from_slice(&frame[copied..copied + count]);
                copied += count;
            }
            let next = if control & END_OF_RING != 0 {
                self.receive_list
            } else if chained {
                second_buffer & !3
            } else {
                self.current_rx
                    .wrapping_add(16 + ((self.bus_mode >> 2) & 31) * 4)
            };
            self.current_rx = next;
            if copied == frame.len() {
                result |= RX_LAST | ((copied as u32) << 16);
                write_descriptor_status(bus, address, result);
                self.rx_state = 3;
                if control & RX_DISABLE_INTERRUPT == 0 {
                    self.status |= RX_COMPLETE;
                }
                return;
            }
            if u64::from(next) == address || processed == MAX_DESCRIPTORS_PER_POLL {
                write_descriptor_status(bus, address, result | RX_LAST | RX_DESCRIPTOR_ERROR);
                self.rx_unavailable();
                return;
            }
            match descriptor(bus, u64::from(next)) {
                Some([status, ..]) if status & DESCRIPTOR_OWN != 0 => {
                    write_descriptor_status(bus, address, result);
                }
                Some(_) => {
                    write_descriptor_status(bus, address, result | RX_LAST | RX_DESCRIPTOR_ERROR);
                    self.rx_unavailable();
                    return;
                }
                None => {
                    write_descriptor_status(bus, address, result | RX_LAST | RX_DESCRIPTOR_ERROR);
                    self.rx_fault();
                    return;
                }
            }
        }
    }

    fn rx_unavailable(&mut self) {
        self.rx_state = 4;
        self.status |= RX_UNAVAILABLE;
    }

    fn rx_fault(&mut self) {
        self.rx_state = 0;
        self.operation &= !DMA_START_RX;
        self.status |= FATAL_BUS_ERROR;
    }

    fn tx_fault(&mut self) {
        self.tx_state = 0;
        self.operation &= !DMA_START_TX;
        self.status |= FATAL_BUS_ERROR;
    }
}

fn descriptor(bus: &Bus, address: u64) -> Option<[u32; 4]> {
    let bytes = bus.ram64(address, 16)?;
    Some(std::array::from_fn(|word| {
        let offset = word * 4;
        u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
    }))
}

fn write_descriptor_status(bus: &mut Bus, address: u64, status: u32) {
    bus.memory64(address, 4)
        .expect("descriptor was read from RAM")
        .copy_from_slice(&status.to_le_bytes());
}

fn ethernet_crc(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

impl Bus {
    pub fn has_rv64_ethernet_link(&self) -> bool {
        self.rv64_devices
            .ethernet
            .iter()
            .any(Ethernet::has_link)
    }

    pub(crate) fn connect_local_rv64_ethernet(&mut self, switch: &LocalSwitch) -> Result<(), String> {
        let [mac] = self.rv64_devices.ethernet.as_mut_slice() else {
            return Err("local Ethernet switch requires exactly one DesignWare MAC".into());
        };
        if mac.has_link() {
            return Err("Ethernet is already connected".into());
        }
        mac.local = Some(switch.connect());
        Ok(())
    }

    pub fn connect_rv64_ethernet(&mut self, local: u16, peer: u16) -> Result<(), String> {
        let [mac] = self.rv64_devices.ethernet.as_mut_slice() else {
            return Err("--ethernet-loopback requires exactly one BeagleV Ethernet MAC".into());
        };
        let link = UdpSocket::bind((Ipv4Addr::LOCALHOST, local))
            .map_err(|error| format!("bind Ethernet loopback port {local}: {error}"))?;
        link.connect((Ipv4Addr::LOCALHOST, peer))
            .and_then(|()| link.set_nonblocking(true))
            .map_err(|error| format!("connect Ethernet loopback port {peer}: {error}"))?;
        mac.link = Some(link);
        Ok(())
    }

    pub fn poll_rv64_ethernet(&mut self) {
        let mut macs = std::mem::take(&mut self.rv64_devices.ethernet);
        for mac in &mut macs {
            mac.poll_link(self);
            self.rv64_devices
                .set_ethernet_irq(mac.irq, mac.interrupt_pending());
        }
        self.rv64_devices.ethernet = macs;
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rv64_devices::Plic;

    const MAC: u32 = 0x1002_0000;
    const PLIC: u32 = 0x0c00_0000;
    const IRQ: u32 = 6;
    const RING: u32 = 0x8000_0000;

    fn write(bus: &mut Bus, offset: u32, value: u32) {
        assert!(bus.write_rv64_ethernet(MAC + offset, 4, value));
    }

    fn read(bus: &mut Bus, offset: u32) -> u32 {
        bus.rv64_devices.read(MAC + offset, 4).unwrap()
    }

    fn put_descriptor(bus: &mut Bus, address: u32, words: [u32; 4]) {
        let bytes = bus.memory64(u64::from(address), 16).unwrap();
        for (word, slot) in words.into_iter().zip(bytes.chunks_exact_mut(4)) {
            slot.copy_from_slice(&word.to_le_bytes());
        }
    }

    fn configure_plic(bus: &mut Bus) {
        bus.rv64_devices.plic = Some(Plic::new(PLIC, 31, vec![Some((0, 9))], true));
        for (offset, value) in [(IRQ * 4, 1), (0x2000, 1 << IRQ)] {
            bus.rv64_devices
                .write(PLIC + offset, 4, value, &mut Vec::new())
                .unwrap()
                .unwrap();
        }
    }

    #[test]
    fn ethernet_snapshot_preserves_dma_and_phy_state() {
        let mut ethernet = Ethernet::new(0x1002_0000, 5);
        ethernet.attach(0, Phy::default());
        ethernet.phys[0].as_mut().unwrap().registers[4] = 0x01e1;
        ethernet.registers.current_rx = 0x8000_8000;
        ethernet.registers.status = 0x10040;
        ethernet.tx_frame.extend([1, 2, 3]);
        let mut snapshot = Vec::new();
        ethernet.save_snapshot(&mut snapshot).unwrap();

        let mut initial = Ethernet::new(0x1002_0000, 5);
        assert!(initial.read_snapshot(&mut snapshot.as_slice()).is_err());
        initial.attach(0, Phy::default());
        let restored = initial.read_snapshot(&mut snapshot.as_slice()).unwrap();
        assert_eq!(restored.phys[0].unwrap().registers[4], 0x01e1);
        assert_eq!(restored.registers.current_rx, 0x8000_8000);
        assert_eq!(restored.registers.status, 0x10040);
        assert_eq!(restored.tx_frame, [1, 2, 3]);
    }

    #[test]
    fn transmit_poll_returns_descriptors_and_raises_plic_interrupt() {
        let mut bus = Bus::new(4096).unwrap();
        bus.rv64_devices
            .ethernet
            .push(Ethernet::new(MAC, IRQ as usize));
        configure_plic(&mut bus);
        write(&mut bus, DMA_TX_LIST, RING);
        write(&mut bus, DMA_INTERRUPT_ENABLE, NORMAL_SUMMARY | TX_COMPLETE);
        write(&mut bus, MAC_CONFIG, MAC_TX_ENABLE);
        write(&mut bus, DMA_OPERATION, DMA_START_TX);
        assert_eq!(read(&mut bus, DMA_STATUS), (6 << 20) | TX_UNAVAILABLE);

        let second = RING + 32;
        let buffer = RING + 128;
        let first = [DESCRIPTOR_OWN, CHAINED | 64, buffer, second];
        let last = [
            DESCRIPTOR_OWN,
            END_OF_RING | LAST_SEGMENT | INTERRUPT_ON_COMPLETION | 64,
            buffer,
            0,
        ];
        for (address, descriptor) in [(RING, first), (second, last)] {
            put_descriptor(&mut bus, address, descriptor);
        }
        write(&mut bus, DMA_TX_POLL, 0);
        for address in [RING, second] {
            assert_eq!(bus.ram64(u64::from(address), 4).unwrap(), [0; 4]);
        }
        assert_eq!(read(&mut bus, DMA_CURRENT_TX), RING);
        let status = read(&mut bus, DMA_STATUS);
        assert_eq!(
            status,
            (6 << 20) | TX_UNAVAILABLE | TX_COMPLETE | NORMAL_SUMMARY
        );
        let claim = PLIC + 0x20_0004;
        assert_eq!(bus.rv64_devices.tick(0), 1 << 9);
        assert_eq!(bus.rv64_devices.read(claim, 4), Some(IRQ));
        write(&mut bus, DMA_STATUS, status);
        bus.rv64_devices
            .write(claim, 4, IRQ, &mut Vec::new())
            .unwrap()
            .unwrap();
        assert_eq!(read(&mut bus, DMA_STATUS), 6 << 20);
        assert_eq!(bus.rv64_devices.tick(0), 0);
    }

    #[test]
    fn loopback_transfers_multisegment_frames_to_rx_ring_and_signals_plic() {
        let first = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let second = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        first.connect(second.local_addr().unwrap()).unwrap();
        second.connect(first.local_addr().unwrap()).unwrap();
        first.set_nonblocking(true).unwrap();
        second.set_nonblocking(true).unwrap();
        let mut tx = Bus::new(4096).unwrap();
        let mut rx = Bus::new(4096).unwrap();
        for (bus, link) in [(&mut tx, first), (&mut rx, second)] {
            let mut mac = Ethernet::new(MAC, IRQ as usize);
            mac.link = Some(link);
            bus.rv64_devices.ethernet.push(mac);
        }
        configure_plic(&mut rx);
        let mut frame = [0x5a; 60];
        frame[..6].fill(0xff);
        let data = RING + 128;
        tx.memory64(u64::from(data), frame.len())
            .unwrap()
            .copy_from_slice(&frame);
        put_descriptor(&mut tx, RING, [DESCRIPTOR_OWN, FIRST_SEGMENT | 20, data, 0]);
        put_descriptor(
            &mut tx,
            RING + 16,
            [
                DESCRIPTOR_OWN,
                LAST_SEGMENT | END_OF_RING | 40,
                data + 20,
                0,
            ],
        );
        put_descriptor(
            &mut rx,
            RING,
            [DESCRIPTOR_OWN, 16 | (16 << 11), data, data + 16],
        );
        put_descriptor(
            &mut rx,
            RING + 16,
            [DESCRIPTOR_OWN, END_OF_RING | 64, data + 32, 0],
        );
        write(&mut rx, DMA_RX_LIST, RING);
        write(&mut rx, DMA_INTERRUPT_ENABLE, NORMAL_SUMMARY | RX_COMPLETE);
        write(&mut rx, MAC_CONFIG, MAC_RX_ENABLE);
        write(&mut rx, DMA_OPERATION, DMA_START_RX);
        write(&mut tx, DMA_TX_LIST, RING);
        write(&mut tx, MAC_CONFIG, MAC_TX_ENABLE);
        write(&mut tx, DMA_OPERATION, DMA_START_TX);
        rx.poll_rv64_ethernet();

        assert_eq!(rx.ram64(u64::from(data), frame.len()).unwrap(), frame);
        assert_eq!(
            rx.ram64(u64::from(data) + 60, 4).unwrap(),
            ethernet_crc(&frame).to_le_bytes(),
        );
        assert_eq!(ethernet_crc(b"123456789"), 0xcbf4_3926);
        assert_eq!(descriptor(&rx, RING.into()).unwrap()[0], RX_FIRST);
        assert_eq!(
            descriptor(&rx, (RING + 16).into()).unwrap()[0],
            RX_LAST | (64 << 16)
        );
        assert_eq!(read(&mut rx, DMA_CURRENT_RX), RING);
        assert_eq!(
            read(&mut rx, DMA_STATUS),
            (3 << 17) | RX_COMPLETE | NORMAL_SUMMARY
        );
        assert_eq!(rx.rv64_devices.tick(0), 1 << 9);
        let claim = PLIC + 0x20_0004;
        assert_eq!(rx.rv64_devices.read(claim, 4), Some(IRQ));
        write(&mut rx, DMA_STATUS, RX_COMPLETE | NORMAL_SUMMARY);
        rx.rv64_devices
            .write(claim, 4, IRQ, &mut Vec::new())
            .unwrap()
            .unwrap();
        assert_eq!(rx.rv64_devices.tick(0), 0);
    }

    #[test]
    fn rx_ring_exhaustion_reports_partial_frame_and_can_resume() {
        let mut bus = Bus::new(4096).unwrap();
        let data = RING + 128;
        let mut mac = Ethernet::new(MAC, 0);
        mac.write(MAC + DMA_RX_LIST, RING, &mut bus);
        mac.write(MAC + DMA_OPERATION, DMA_START_RX, &mut bus);
        mac.write(
            MAC + DMA_INTERRUPT_ENABLE,
            ABNORMAL_SUMMARY | RX_UNAVAILABLE,
            &mut bus,
        );
        put_descriptor(
            &mut bus,
            RING,
            [DESCRIPTOR_OWN, CHAINED | 16, data, RING + 32],
        );
        mac.registers.receive(&mut bus, &[0xab; 32]);
        mac.update_interrupt_summary();
        assert_eq!(
            descriptor(&bus, RING.into()).unwrap()[0],
            RX_FIRST | RX_LAST | RX_DESCRIPTOR_ERROR
        );
        assert_eq!(mac.read(MAC + DMA_CURRENT_RX), RING + 32);
        assert_eq!(
            mac.read(MAC + DMA_STATUS),
            (4 << 17) | ABNORMAL_SUMMARY | RX_UNAVAILABLE
        );
        assert!(mac.interrupt_pending());
        mac.write(
            MAC + DMA_STATUS,
            ABNORMAL_SUMMARY | RX_UNAVAILABLE,
            &mut bus,
        );

        put_descriptor(
            &mut bus,
            RING + 32,
            [DESCRIPTOR_OWN, (1 << 31) | END_OF_RING | 64, data, 0],
        );
        mac.write(MAC + DMA_RX_POLL, 0, &mut bus);
        mac.registers.receive(&mut bus, &[0xcd; 32]);
        assert_eq!(
            descriptor(&bus, (RING + 32).into()).unwrap()[0],
            RX_FIRST | RX_LAST | (32 << 16)
        );
        assert_eq!(mac.read(MAC + DMA_CURRENT_RX), RING);
        assert_eq!(mac.read(MAC + DMA_STATUS), 3 << 17);
        assert!(!mac.interrupt_pending());
    }

    #[test]
    fn unmapped_descriptor_stops_dma_and_reset_clears_error() {
        let mut bus = Bus::new(4096).unwrap();
        bus.rv64_devices.ethernet.push(Ethernet::new(MAC, 0));
        write(&mut bus, DMA_TX_LIST, RING + 4096);
        write(
            &mut bus,
            DMA_INTERRUPT_ENABLE,
            ABNORMAL_SUMMARY | FATAL_BUS_ERROR,
        );
        write(&mut bus, MAC_CONFIG, MAC_TX_ENABLE);
        write(&mut bus, DMA_OPERATION, DMA_START_TX);
        assert_eq!(read(&mut bus, DMA_OPERATION) & DMA_START_TX, 0);
        assert_eq!(
            read(&mut bus, DMA_STATUS),
            FATAL_BUS_ERROR | ABNORMAL_SUMMARY
        );
        write(&mut bus, DMA_BUS_MODE, 1);
        assert_eq!(read(&mut bus, DMA_STATUS), 0);
        assert_eq!(read(&mut bus, DMA_INTERRUPT_ENABLE), 0);
        assert_eq!(read(&mut bus, DMA_CURRENT_TX), 0);
    }

    #[test]
    fn dma_reset_discards_queued_frames_and_receives_on_the_new_ring() {
        let link = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let peer = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        link.connect(peer.local_addr().unwrap()).unwrap();
        peer.connect(link.local_addr().unwrap()).unwrap();
        link.set_nonblocking(true).unwrap();
        let mut bus = Bus::new(4096).unwrap();
        let mut mac = Ethernet::new(MAC, 0);
        mac.link = Some(link);
        bus.rv64_devices.ethernet.push(mac);
        let mut frame = [0x64; 60];
        frame[..6].fill(0xff);
        for _ in 0..40 {
            peer.send(&frame).unwrap();
        }
        write(&mut bus, DMA_BUS_MODE, 1);
        assert_eq!(read(&mut bus, DMA_BUS_MODE) & 1, 0);
        put_descriptor(
            &mut bus,
            RING,
            [DESCRIPTOR_OWN, END_OF_RING | 64, RING + 128, 0],
        );
        write(&mut bus, DMA_RX_LIST, RING);
        write(&mut bus, MAC_CONFIG, MAC_RX_ENABLE);
        write(&mut bus, DMA_OPERATION, DMA_START_RX);
        assert_eq!(descriptor(&bus, RING.into()).unwrap()[0], DESCRIPTOR_OWN);
        assert_eq!(read(&mut bus, DMA_STATUS), 3 << 17);

        frame[14] = 0x77;
        peer.send(&frame).unwrap();
        bus.poll_rv64_ethernet();
        assert_eq!(
            descriptor(&bus, RING.into()).unwrap()[0],
            RX_FIRST | RX_LAST | (64 << 16)
        );
        assert_eq!(
            bus.ram64(u64::from(RING + 128), frame.len()).unwrap(),
            frame
        );
    }
}

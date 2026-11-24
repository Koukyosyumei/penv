use std::{cell::RefCell, rc::Rc};

use crate::{
    bus_translation::{NativePeripheral, PeripheralHandle, Port},
    dma_engine::{DmaBus, Width},
    machine::Machine,
    opentitan_spi_device::SpiDevice,
};

const BASE: u64 = 0x1000;
const CONTROL: u64 = 0x10;
const FIFO_DEPTH: u64 = 0x1c;
const RX_POINTERS: u64 = 0x24;
const TX_POINTERS: u64 = 0x28;
const RX_ADDRESSES: u64 = 0x2c;
const TX_ADDRESSES: u64 = 0x30;
const BUFFER: u64 = 0x1000;

struct Attachment(Rc<RefCell<SpiDevice>>);

impl NativePeripheral for Attachment {
    fn read(&mut self, region: Option<&str>, offset: u64) -> u64 {
        self.0.borrow_mut().read(region, offset)
    }
    fn write(&mut self, region: Option<&str>, offset: u64, value: u64) {
        self.0.borrow_mut().write(region, offset, value);
    }
    fn read_width(&mut self, region: Option<&str>, offset: u64, width: Width) -> u64 {
        self.0.borrow_mut().read_width(region, offset, width)
    }
    fn write_width(&mut self, region: Option<&str>, offset: u64, width: Width, value: u64) {
        self.0
            .borrow_mut()
            .write_width(region, offset, width, value);
    }
}

struct Fixture {
    machine: Machine,
    host: Rc<RefCell<SpiDevice>>,
}

impl Fixture {
    fn new() -> Self {
        let mut machine = Machine::without_default_ram();
        let host = Rc::new(RefCell::new(SpiDevice::default()));
        machine
            .bus
            .register_translated_port_widths(
                &PeripheralHandle::default(),
                vec![(
                    Port {
                        base: BASE as u32,
                        length: 0x2000,
                        region: None,
                    },
                    vec![Width::DoubleWord, Width::Byte],
                )],
                Box::new(Attachment(Rc::clone(&host))),
            )
            .unwrap();
        Self { machine, host }
    }

    fn read(&mut self, register: u64) -> u32 {
        self.machine
            .bus
            .read_unit(BASE + register, Width::DoubleWord)
            .unwrap() as u32
    }

    fn write(&mut self, register: u64, value: u32) {
        self.machine
            .bus
            .write_unit(
                BASE + register,
                Width::DoubleWord,
                value.into(),
                &mut std::io::sink(),
            )
            .unwrap();
    }

    fn send(&self, hex: &str) {
        let mut host = self.host.borrow_mut();
        for index in (0..hex.len()).step_by(2) {
            host.transmit(u8::from_str_radix(&hex[index..index + 2], 16).unwrap());
        }
        host.finish_transmission();
    }

    fn receive(&self, len: usize) -> Vec<u8> {
        (0..len)
            .map(|_| self.host.borrow_mut().transmit(0))
            .collect()
    }

    fn append(&mut self, hex: &str) {
        let start = u64::from(self.read(TX_ADDRESSES) & 0xffff) * 4;
        let pointers = self.read(TX_POINTERS);
        let mut pointer = pointers >> 16;
        for index in (0..hex.len()).step_by(2) {
            let byte = u8::from_str_radix(&hex[index..index + 2], 16).unwrap();
            self.machine
                .bus
                .write_unit(
                    BASE + BUFFER + start + u64::from(pointer),
                    Width::Byte,
                    byte.into(),
                    &mut std::io::sink(),
                )
                .unwrap();
            pointer += 1;
        }
        self.write(TX_POINTERS, (pointers & 0xffff) | pointer << 16);
    }
}

#[test]
fn should_have_correct_fifo_configuration_after_boot() {
    let mut fixture = Fixture::new();
    fixture.write(CONTROL, 0);
    let rx = fixture.read(RX_ADDRESSES);
    let tx = fixture.read(TX_ADDRESSES);
    assert_eq!((rx & 0xffff) * 4, 0);
    assert_eq!(((rx >> 16) + 4) * 4 - 1, 2047);
    assert_eq!((tx & 0xffff) * 4, 2048);
    assert_eq!(((tx >> 16) + 4) * 4 - 1, 4095);
}

#[test]
fn should_receive_data_to_fifo() {
    let mut fixture = Fixture::new();
    assert_eq!(fixture.read(RX_POINTERS), 0);
    fixture.write(CONTROL, 1 << 31);
    fixture.send("deadbeef");
    assert_eq!(fixture.read(RX_POINTERS) >> 16, 4);
    assert_eq!(fixture.read(FIFO_DEPTH), 4);
}

#[test]
fn should_be_able_to_transmit_data() {
    let mut fixture = Fixture::new();
    fixture.append("deadbeef");
    assert_eq!(fixture.read(FIFO_DEPTH) >> 16, 4);
    assert_eq!(fixture.receive(4), [0xde, 0xad, 0xbe, 0xef]);
}

#[test]
fn should_be_able_to_reset_fifos() {
    let mut fixture = Fixture::new();
    fixture.send("deadbeef");
    fixture.append("deadbeef");
    let level = fixture.read(FIFO_DEPTH);
    assert_eq!(level & 0xffff, 4);
    assert_eq!(level >> 16, 4);
    fixture.write(CONTROL, 3 << 16);
    let level = fixture.read(FIFO_DEPTH);
    assert_eq!(level & 0xffff, 0);
    assert_eq!(level >> 16, 0);
}

const INTERRUPT_ENABLE: u64 = 4;
const WATERMARK: u64 = 0x18;
const RX_FULL: u32 = 1;
const RX_WATERMARK: u32 = 1 << 1;
const TX_WATERMARK: u32 = 1 << 2;
const RX_OVERFLOW: u32 = 1 << 4;
const TX_UNDERFLOW: u32 = 1 << 5;
const MAX_FIFO_CAPACITY: usize = 2048;

#[test]
fn should_set_interrupt_on_rx_fifo_full() {
    let mut fixture = Fixture::new();
    fixture.write(INTERRUPT_ENABLE, RX_FULL);
    for _ in 0..MAX_FIFO_CAPACITY - 1 {
        fixture.send("FF");
    }
    assert!(!fixture.host.borrow().interrupt(RX_FULL));
    fixture.send("FF");
    assert!(fixture.host.borrow().interrupt(RX_FULL));
}

#[test]
fn should_set_interrupt_on_rx_overflow() {
    let mut fixture = Fixture::new();
    fixture.write(INTERRUPT_ENABLE, RX_OVERFLOW);
    for _ in 0..MAX_FIFO_CAPACITY {
        fixture.send("FF");
    }
    assert!(!fixture.host.borrow().interrupt(RX_OVERFLOW));
    fixture.send("FF");
    assert!(fixture.host.borrow().interrupt(RX_OVERFLOW));
}

#[test]
fn should_set_interrupt_on_rx_fifo_watermark() {
    let mut fixture = Fixture::new();
    fixture.write(INTERRUPT_ENABLE, RX_WATERMARK);
    for _ in 0..0x80 {
        fixture.send("FF");
    }
    assert!(!fixture.host.borrow().interrupt(RX_WATERMARK));
    fixture.send("FF");
    assert!(fixture.host.borrow().interrupt(RX_WATERMARK));
}

#[test]
fn should_set_interrupt_on_tx_fifo_watermark() {
    let mut fixture = Fixture::new();
    fixture.write(INTERRUPT_ENABLE, TX_WATERMARK);
    fixture.write(WATERMARK, 0x80 << 16);
    fixture.write(TX_POINTERS, 0x80 << 16);
    assert_eq!(fixture.read(FIFO_DEPTH) >> 16, 0x80);
    assert!(!fixture.host.borrow().interrupt(TX_WATERMARK));
    fixture.host.borrow_mut().transmit(0);
    assert!(fixture.host.borrow().interrupt(TX_WATERMARK));
}

#[test]
fn should_set_interrupt_on_tx_underflow() {
    let mut fixture = Fixture::new();
    fixture.write(INTERRUPT_ENABLE, TX_UNDERFLOW);
    fixture.host.borrow_mut().transmit(0);
    assert!(fixture.host.borrow().interrupt(TX_UNDERFLOW));
}

use std::{cell::RefCell, collections::VecDeque, rc::Rc};

use crate::{
    bus_translation::{NativePeripheral, Port},
    dma_engine::{DmaBus, Width},
    efr32_i2c::{Efr32I2c, Slave},
    machine::Machine,
    nrf52840::bmp180::Bmp180,
};

const BASE: u64 = 0x4a01_0000;
const RX_ADDRESS: u8 = 0x10;
const TX_ADDRESS: u8 = 0x20;
const MAGIC: [u8; 16] = [
    0x11, 0x22, 0x33, 0x44, 0xff, 0xee, 0xdd, 0xcc, 0x11, 0x22, 0x33, 0x44, 0xff, 0xee, 0xdd, 0xcc,
];

#[derive(Default)]
struct Dummy(VecDeque<u8>);
impl Slave for Dummy {
    fn write(&mut self, _: &[u8]) {}
    fn read(&mut self, count: usize) -> Vec<u8> {
        let count = count.min(self.0.len());
        self.0.drain(..count).collect()
    }
}

#[derive(Default)]
struct Echo(VecDeque<u8>);
impl Slave for Echo {
    fn write(&mut self, bytes: &[u8]) {
        self.0.extend(bytes);
    }
    fn read(&mut self, count: usize) -> Vec<u8> {
        let count = count.min(self.0.len());
        self.0.drain(..count).collect()
    }
}

struct RegisteredController(Rc<RefCell<Efr32I2c>>);
impl NativePeripheral for RegisteredController {
    fn read(&mut self, region: Option<&str>, offset: u64) -> u64 {
        self.0.borrow_mut().read(region, offset)
    }
    fn write(&mut self, region: Option<&str>, offset: u64, value: u64) {
        self.0.borrow_mut().write(region, offset, value);
    }
}

struct Fixture {
    machine: Machine,
    controller: Rc<RefCell<Efr32I2c>>,
    dummy: Rc<RefCell<Dummy>>,
    echo: Rc<RefCell<Echo>>,
}
impl Fixture {
    fn new() -> Self {
        let mut machine = Machine::without_default_ram();
        let controller = Rc::new(RefCell::new(Efr32I2c::default()));
        let dummy = Rc::new(RefCell::new(Dummy::default()));
        let echo = Rc::new(RefCell::new(Echo::default()));
        {
            let mut controller = controller.borrow_mut();
            controller.reset();
            controller.write(None, 0, 1 | (1 << 2));
            controller.write(None, 0x40, 0x7ffff);
        }
        machine
            .bus
            .map_translated_peripheral(
                vec![Port {
                    base: BASE as u32,
                    length: 0x400,
                    region: None,
                }],
                Width::DoubleWord,
                Box::new(RegisteredController(controller.clone())),
            )
            .unwrap();
        controller.borrow_mut().register(RX_ADDRESS, dummy.clone());
        controller.borrow_mut().register(TX_ADDRESS, echo.clone());
        Self {
            machine,
            controller,
            dummy,
            echo,
        }
    }

    fn read(&mut self, offset: u64) -> u64 {
        self.machine
            .bus
            .read_unit(BASE + offset, Width::DoubleWord)
            .unwrap()
    }

    fn write(&mut self, offset: u64, value: u64) {
        self.machine
            .bus
            .write_unit(
                BASE + offset,
                Width::DoubleWord,
                value,
                &mut std::io::sink(),
            )
            .unwrap();
    }

    fn begin(&mut self, address: u8, read: bool) {
        self.write(4, 1);
        self.write(0x2c, u64::from((address << 1) | u8::from(read)));
    }

    fn end(&mut self) {
        self.write(4, 2);
    }

    fn receive(&mut self, count: usize, double: bool) -> Vec<u8> {
        let mut bytes = Vec::new();
        while bytes.len() < count && self.read(0xc) & 0x100 != 0 {
            if double {
                assert_eq!(self.read(0xc) & (1 << 9), 1 << 9);
                bytes.extend((self.read(0x20) as u16).to_le_bytes());
            } else {
                assert_eq!(self.read(0xc) & 0x100, 0x100);
                bytes.push(self.read(0x1c) as u8);
            }
        }
        bytes
    }

    fn prepare_read(&mut self) {
        self.dummy.borrow_mut().0.extend(MAGIC);
        self.begin(RX_ADDRESS, true);
    }

    fn check_interrupt(&mut self, mask: u64) {
        assert_eq!(self.read(0x34) & mask, mask);
    }

    fn write_magic(&mut self, double: bool) {
        self.begin(TX_ADDRESS, false);
        if double {
            for pair in MAGIC.chunks_exact(2) {
                self.write(0x30, u16::from_le_bytes([pair[0], pair[1]]).into());
            }
        } else {
            for byte in MAGIC {
                self.write(0x2c, byte.into());
            }
        }
        self.end();
        assert_eq!(self.echo.borrow_mut().read(MAGIC.len()), MAGIC);
    }

    fn read_magic(&mut self, double: bool) {
        self.prepare_read();
        let bytes = self.receive(MAGIC.len(), double);
        self.end();
        assert_eq!(bytes, MAGIC);
    }
}

#[test]
fn write_to_slave_using_single_byte_transfer() {
    Fixture::new().write_magic(false);
}
#[test]
fn write_to_slave_using_double_byte_transfer() {
    Fixture::new().write_magic(true);
}
#[test]
fn read_from_slave_using_single_byte_transfer() {
    Fixture::new().read_magic(false);
}
#[test]
fn read_from_slave_using_double_byte_transfer() {
    Fixture::new().read_magic(true);
}
#[test]
fn peek_single_byte() {
    let mut f = Fixture::new();
    f.prepare_read();
    assert_eq!(f.read(0x24), MAGIC[0].into());
    f.end();
}
#[test]
fn peek_double_byte() {
    let mut f = Fixture::new();
    f.prepare_read();
    let [first, second] = (f.read(0x28) as u16).to_le_bytes();
    assert_eq!(first, MAGIC[0]);
    assert_eq!(second, MAGIC[1]);
    f.end();
}
#[test]
fn interrupt_read_underflow() {
    let mut f = Fixture::new();
    f.read(0x1c);
    f.check_interrupt(1 << 13);
}
#[test]
fn interrupt_master_stop() {
    let mut f = Fixture::new();
    f.end();
    f.check_interrupt(1 << 8);
}
#[test]
fn interrupt_start() {
    let mut f = Fixture::new();
    f.write(4, 1);
    f.check_interrupt(1);
}
#[test]
fn interrupt_ack() {
    let mut f = Fixture::new();
    f.begin(RX_ADDRESS, true);
    f.check_interrupt(1 << 6);
}
#[test]
fn interrupt_nack() {
    let mut f = Fixture::new();
    f.begin(0x40, true);
    f.check_interrupt(1 << 7);
}
#[test]
fn interrupt_bus_hold() {
    let mut f = Fixture::new();
    f.begin(RX_ADDRESS, true);
    f.check_interrupt(1 << 11);
    f.end();
}
#[test]
fn interrupt_transmit_buffer_level() {
    let mut f = Fixture::new();
    f.begin(TX_ADDRESS, false);
    f.write(0x2c, 0x42);
    f.end();
    f.check_interrupt(1 << 4);
    f.write(0x2c, 0x42);
    assert_eq!(f.read(0x34) & (1 << 4), 0);
}
#[test]
fn interrupt_transfer_completed() {
    let mut f = Fixture::new();
    f.begin(TX_ADDRESS, false);
    f.write(0x2c, 0x42);
    f.end();
    f.check_interrupt(1 << 3);
}
#[test]
fn test_with_bmp180() {
    let mut f = Fixture::new();
    let mut sensor = Bmp180::new(0.0, 0);
    sensor.reset();
    f.controller
        .borrow_mut()
        .register(0x77, Rc::new(RefCell::new(sensor)));
    f.begin(0x77, false);
    f.write(0x2c, 0xf6);
    f.end();
    f.begin(0x77, true);
    let bytes = f.receive(8, false);
    f.end();
    assert!(!bytes.is_empty());
    assert_eq!(bytes[0], 0x80);
}

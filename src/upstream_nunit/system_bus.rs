use crate::{constant_mmio::ConstantMmio, machine::Machine};

#[test]
fn find_after_registration() {
    let mut machine = Machine::without_default_ram();
    machine
        .bus
        .map_constant_mmio(ConstantMmio::doubleword(1000, 1000, 0x666).unwrap())
        .unwrap();
    assert_eq!(machine.bus.read_scalar::<4>(1000, true).unwrap(), 0x666);
}

#[test]
fn find_after_many_registrations() {
    let mut machine = Machine::without_default_ram();
    let peripherals = [(1000, 0x666), (2000, 0x667), (3000, 0x668)];
    for (base, value) in peripherals {
        machine
            .bus
            .map_constant_mmio(ConstantMmio::doubleword(base, 100, value).unwrap())
            .unwrap();
    }
    for (base, value) in peripherals {
        assert_eq!(machine.bus.read_scalar::<4>(base, true).unwrap(), value);
    }
}

#[test]
fn should_test_enable_all_translations() {
    use crate::{
        bus_translation::{NativePeripheral, Port},
        dma_engine::{DmaBus, Width},
    };

    #[derive(Default)]
    struct Peripheral {
        registers: [u64; 8],
        region_registers: [u64; 8],
    }
    impl NativePeripheral for Peripheral {
        fn read(&mut self, region: Option<&str>, offset: u64) -> u64 {
            match region {
                None => self.registers[offset as usize],
                Some("region") => self.region_registers[offset as usize],
                Some(name) => panic!("unexpected connection region {name}"),
            }
        }
        fn write(&mut self, region: Option<&str>, offset: u64, value: u64) {
            match region {
                None => self.registers[offset as usize] = value,
                Some("region") => self.region_registers[offset as usize] = value,
                Some(name) => panic!("unexpected connection region {name}"),
            }
        }
    }

    let mut machine = Machine::without_default_ram();
    let bus = &mut machine.bus;
    let widths = [Width::QuadWord, Width::DoubleWord, Width::Word, Width::Byte];
    for (index, native) in widths.into_iter().enumerate() {
        let base = index as u32 * 16;
        bus.map_translated_peripheral(
            vec![
                Port {
                    base,
                    length: 8,
                    region: None,
                },
                Port {
                    base: base + 8,
                    length: 8,
                    region: Some("region".into()),
                },
            ],
            native,
            Box::new(Peripheral::default()),
        )
        .unwrap();
    }

    let mut output = std::io::sink();
    let pattern = 0xdeadc0de12345678_u64;
    for (index, native) in widths.into_iter().enumerate() {
        for address in [index as u64 * 16, index as u64 * 16 + 8] {
            for requested in widths.into_iter().rev().filter(|&width| width != native) {
                let width = requested as usize;
                bus.enable_all_translations(true);
                for offset in (0..8).step_by(width) {
                    let value = (pattern >> (offset * 8)) & (u64::MAX >> (64 - width * 8));
                    let address = address + offset as u64;
                    bus.write_unit(address, requested, value, &mut output)
                        .unwrap();
                    assert_eq!(bus.read_unit(address, requested).unwrap(), value);
                }
                bus.enable_all_translations(false);
                for offset in (0..8).step_by(width) {
                    assert_eq!(
                        bus.read_unit(address + offset as u64, requested).unwrap(),
                        0
                    );
                }
                bus.enable_all_translations(true);
                bus.write_unit(address, Width::QuadWord, 0, &mut output)
                    .unwrap();
                assert_eq!(bus.read_unit(address, Width::QuadWord).unwrap(), 0);
            }
        }
    }
}

use crate::{
    bus_translation::{NativePeripheral, PeripheralHandle, Port},
    dma_engine::{DmaBus, Width},
};
use std::{cell::RefCell, rc::Rc};

struct CallbackPeripheral(Rc<RefCell<[bool; 6]>>);

impl CallbackPeripheral {
    fn callback(&self, region: Option<&str>, width: Width, write: bool) {
        let index = match (region, width, write) {
            (Some("region1" | "parameterized"), Width::Byte, false) => 0,
            (Some("region2"), Width::Byte, false) => 1,
            (Some("region1" | "parameterized"), Width::Byte, true) => 2,
            (Some("region2"), Width::Byte, true) => 3,
            (None | Some("parameterized"), Width::DoubleWord, false) => 4,
            (None | Some("parameterized"), Width::DoubleWord, true) => 5,
            _ => panic!("unexpected port and width combination"),
        };
        self.0.borrow_mut()[index] = true;
    }
}

impl NativePeripheral for CallbackPeripheral {
    fn read(&mut self, region: Option<&str>, offset: u64) -> u64 {
        self.read_width(region, offset, Width::DoubleWord)
    }

    fn write(&mut self, region: Option<&str>, offset: u64, value: u64) {
        self.write_width(region, offset, Width::DoubleWord, value);
    }

    fn read_width(&mut self, region: Option<&str>, _: u64, width: Width) -> u64 {
        self.callback(region, width, false);
        0
    }

    fn write_width(&mut self, region: Option<&str>, _: u64, width: Width, _: u64) {
        self.callback(region, width, true);
    }
}

fn callback_test(
    ports: &[(u32, u32, Option<&str>, &[Width])],
    accesses: &[(u64, Width, bool, usize)],
) {
    let mut machine = Machine::without_default_ram();
    let bus = &mut machine.bus;
    let flags = Rc::new(RefCell::new([false; 6]));
    let ports = ports
        .iter()
        .map(|&(base, length, name, widths)| {
            (
                Port {
                    base,
                    length,
                    region: name.map(str::to_owned),
                },
                widths.to_vec(),
            )
        })
        .collect();
    bus.register_translated_port_widths(
        &PeripheralHandle::default(),
        ports,
        Box::new(CallbackPeripheral(Rc::clone(&flags))),
    )
    .unwrap();
    let mut expected = [false; 6];
    assert_eq!(*flags.borrow(), expected);
    for &(address, width, write, index) in accesses {
        if write {
            bus.write_unit(address, width, 0, &mut std::io::sink())
                .unwrap();
        } else {
            bus.read_unit(address, width).unwrap();
        }
        expected[index] = true;
        assert_eq!(*flags.borrow(), expected);
    }
}

#[test]
fn should_register_multi_function_peripheral() {
    callback_test(
        &[
            (0, 100, Some("region1"), &[Width::Byte]),
            (100, 200, Some("region2"), &[Width::Byte]),
            (300, 100, None, &[Width::DoubleWord]),
        ],
        &[
            (10, Width::Byte, false, 0),
            (110, Width::Byte, false, 1),
            (10, Width::Byte, true, 2),
            (110, Width::Byte, true, 3),
            (310, Width::DoubleWord, false, 4),
            (310, Width::DoubleWord, true, 5),
        ],
    );
}

#[test]
fn should_register_parametrized_peripheral() {
    callback_test(
        &[
            (
                0,
                100,
                Some("parameterized"),
                &[Width::DoubleWord, Width::Byte],
            ),
            (200, 100, None, &[Width::DoubleWord]),
        ],
        &[
            (10, Width::Byte, false, 0),
            (10, Width::Byte, true, 2),
            (10, Width::DoubleWord, false, 4),
            (10, Width::DoubleWord, true, 5),
        ],
    );
}

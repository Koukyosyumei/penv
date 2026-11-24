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

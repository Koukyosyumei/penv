use crate::machine::Machine;

#[test]
fn read_write_memory_bigger_than_2gb() {
    const SIZE: u32 = 3 * 1024 * 1024 * 1024;
    const START: u32 = 100 * 1024 * 1024;
    let mut machine = Machine::without_default_ram();
    machine.bus.map_ram(START, SIZE as usize).unwrap();
    let first = START + 16;
    let last = START + SIZE - 16;
    let mut sink = std::io::sink();

    machine.bus.write(first, 1, 0x1, &mut sink).unwrap();
    machine.bus.write(last, 1, 0x2, &mut sink).unwrap();
    assert_eq!(machine.bus.read::<1>(first).unwrap(), 0x1);
    assert_eq!(machine.bus.read::<1>(last).unwrap(), 0x2);
}

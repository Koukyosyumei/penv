use crate::bma180::Bma180;

fn select(device: &mut Bma180, register: u8) -> &[u8] {
    device.write(&[0xfc, register]).unwrap();
    device.read()
}

#[test]
fn init_test() {
    let mut sensor = Bma180::default();
    sensor.reset();
    assert_eq!(select(&mut sensor, 0)[0], 3);
    assert_eq!(select(&mut sensor, 0x0d)[0], 0);
    sensor.write(&[0xfd, 0x0d, 0x10]).unwrap();
    assert_eq!(select(&mut sensor, 0x0d)[0], 0x10);
    assert_eq!(select(&mut sensor, 0x10)[0], 0);
    sensor.write(&[0xfd, 0x10, 0xb6]).unwrap();
    assert_eq!(select(&mut sensor, 0x0d)[0], 0);
    for (register, value) in [(0x26, 0x50), (0x27, 0x32), (0x29, 0x17), (0x2a, 0x50)] {
        assert_eq!(select(&mut sensor, register)[0], value);
    }
}

fn decode(lsb: u8, msb: u8) -> u16 {
    ((u16::from(msb) << 6) & 0x3fc0) + ((u16::from(lsb) >> 2) & 0x3f)
}

#[test]
fn read_data_test() {
    let mut sensor = Bma180::default();
    sensor.reset();
    assert!(select(&mut sensor, 0x08)[0] > 0);
    let x = select(&mut sensor, 0x02).to_vec();
    assert!((0..=0x3fff).contains(&decode(x[0], x[1])));
    for register in [0x04, 0x06] {
        let axis = select(&mut sensor, register);
        assert!((0..=0x3fff).contains(&decode(axis[0], axis[1])));
    }
    let axes = select(&mut sensor, 0x02);
    assert!((0..=0x3fff).contains(&decode(axes[0], axes[1])));
    assert!((0..=0x3fff).contains(&decode(axes[2], axes[3])));
    // The final upstream expression uses the first X read as its LSB source.
    assert!((0..=0x3fff).contains(&decode(x[1], axes[5])));
}

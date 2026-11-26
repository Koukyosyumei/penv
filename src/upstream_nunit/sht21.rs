use crate::sht21::Sht21;

#[test]
fn read_temperature_test() {
    let mut sensor = Sht21::default();
    sensor.reset();
    sensor.write(&[0xfd, 0xe3]).unwrap();
    sensor.write(&[0xfc, 0xe3]).unwrap();
    let temperature = sensor.read();

    // The upstream fixture calculates its own sensor CRC independently.
    let mut crc = 0u8;
    for byte in [temperature[0], temperature[1]] {
        crc ^= byte;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 {
                ((u16::from(crc) << 1) ^ 0x131) as u8
            } else {
                crc << 1
            };
        }
    }
    assert_eq!(crc, temperature[2]);

    let raw = ((u16::from(temperature[0]) << 6) & 0x3fc0) + (u16::from(temperature[1] & 0xfc) >> 2);
    let degrees = 17572.0 * f64::from(raw) / 65536.0 - 46.85;
    assert!((degrees * 100.0).round_ties_even() / 100.0 > 0.0);
}

#[test]
fn init_test() {
    let mut sensor = Sht21::default();
    sensor.reset();
    sensor.write(&[0xfc, 0xfa, 0x0f]).unwrap();
    assert_eq!(sensor.read()[..9], [0; 9]);
    sensor.write(&[0xfd, 0xfa, 0xc9]).unwrap();
    assert_eq!(sensor.read()[..7], [0; 7]);

    let user = 0xaa;
    sensor.write(&[0xfd, 0xe6, user]).unwrap();
    sensor.write(&[0xfc, 0xe7]).unwrap();
    assert_eq!(sensor.read()[0], user);
    sensor.write(&[0xfd, 0xfe, 0]).unwrap();
    sensor.write(&[0xfc, 0xe7]).unwrap();
    assert_eq!(sensor.read()[0], 0x3a);
}

// Standalone checks for the fixed Nano 33 BLE inputs. The firmware UART
// assertions remain in nrf52840-nano33-upstream.sh.
#[path = "../../src/nrf52840/arducam.rs"]
#[allow(dead_code)]
mod arducam;
#[path = "../../src/nrf52840/lsm9ds1.rs"]
mod lsm9ds1;

use std::{fs, path::PathBuf};

fn input(name: &str) -> PathBuf {
    PathBuf::from(std::env::var_os("PENV_NANO33_DEVICE_INPUTS").expect("fixture directory"))
        .join(name)
}

fn camera_read(camera: &mut arducam::ArduCam, register: u8) -> u8 {
    camera.release();
    assert_eq!(camera.transmit(register), 0);
    camera.transmit(0)
}

fn camera_capture(name: &str, length: usize) {
    let image = fs::read(input(name)).unwrap();
    assert_eq!(image.len(), length);
    let mut camera = arducam::ArduCam::default();
    camera.load_image(image.clone());
    assert_eq!(camera_read(&mut camera, 0x41), 0);
    camera.release();
    camera.transmit(0x84);
    camera.transmit(2);
    assert_eq!(camera_read(&mut camera, 0x41), 8);
    let size = usize::from(camera_read(&mut camera, 0x42))
        | usize::from(camera_read(&mut camera, 0x43)) << 8
        | usize::from(camera_read(&mut camera, 0x44)) << 16;
    assert_eq!(size, image.len());
    camera.release();
    assert_eq!(camera.transmit(0x3c), 0);
    for &byte in &image[..31] {
        assert_eq!(camera.transmit(0), byte);
    }

    // The upstream reset clears register and transaction state, but leaves
    // imageData and imageDataPointer intact.
    camera.reset();
    assert_eq!(camera_read(&mut camera, 0x41), 0);
    camera.release();
    assert_eq!(camera.transmit(0x3c), 0);
    for &byte in &image[31..] {
        assert_eq!(camera.transmit(0), byte);
    }
    assert_eq!(camera.transmit(0), 0);
}

#[test]
fn person_image_0() {
    camera_capture("person_image_0.jpg", 3853);
}
#[test]
fn person_image_1() {
    camera_capture("person_image_1.jpg", 3836);
}
#[test]
fn no_person_image_0() {
    camera_capture("no_person_image_0.jpg", 3787);
}
#[test]
fn no_person_image_1() {
    camera_capture("no_person_image_1.jpg", 3910);
}

fn imu_read(imu: &mut lsm9ds1::Lsm9ds1, register: u8) -> u8 {
    imu.start_write();
    imu.write(register);
    imu.read()
}

fn imu_samples(name: &str) {
    let source = fs::read_to_string(input(name)).unwrap();
    let samples: Vec<[f64; 3]> = source
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            line.split_whitespace()
                .map(|v| v.parse::<f64>().unwrap())
                .collect::<Vec<_>>()
                .try_into()
                .unwrap()
        })
        .collect();
    let mut imu = lsm9ds1::Lsm9ds1::new(false);
    imu.imu_samples().unwrap().extend(samples.iter().copied());
    assert!(samples.iter().flatten().any(|&value| value != 0.0));
    assert_eq!(imu_read(&mut imu, 0x0f), 0x68);
    for (i, sample) in samples.iter().enumerate() {
        assert_eq!(imu_read(&mut imu, 0x2f), 0);
        assert_eq!(imu_read(&mut imu, 0x2f), 1);
        imu.start_write();
        imu.write(0x28);
        let bytes: [u8; 6] = std::array::from_fn(|_| imu.read());
        let expected = sample
            .map(|value| ((value * 16384.0) as i16).to_le_bytes())
            .concat();
        assert_eq!(bytes.as_slice(), expected, "sample {i}");
        if bytes[2..] != [0; 4] {
            imu.reset();
            // Reading Y/Z without a fresh X-low read should keep the latch.
            imu.start_write();
            imu.write(0x2a);
            let latched: [u8; 4] = std::array::from_fn(|_| imu.read());
            assert_eq!(latched, bytes[2..], "sample latch across reset at {i}");
        }
    }
    assert!(imu.imu_samples().unwrap().is_empty());
}

#[test]
fn circle() {
    imu_samples("circle_rotated.data");
}
#[test]
fn angle() {
    imu_samples("angle_rotated.data");
}

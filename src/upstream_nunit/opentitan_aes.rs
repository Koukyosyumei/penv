use crate::{bus_translation::NativePeripheral, opentitan_aes::OpenTitanAes};

const ENCRYPTED: [u32; 4] = [0xcab7a28e, 0xbf456751, 0x9049fcea, 0x8960494b];
const DECRYPTED: [u32; 4] = [0x33221100, 0x77665544, 0xbbaa9988, 0xffeeddcc];
const CONTROL: u64 = 0x74;
const TRIGGER: u64 = 0x80;
const STATUS: u64 = 0x84;

fn setup() -> OpenTitanAes {
    OpenTitanAes::new(1)
}

fn write(aes: &mut OpenTitanAes, offset: u64, value: u32) {
    aes.write(None, offset, value.into());
}

fn input(aes: &mut OpenTitanAes, data: [u32; 4]) {
    for (index, part) in data.into_iter().enumerate() {
        write(aes, 0x54 + 4 * index as u64, part);
    }
}

fn output(aes: &mut OpenTitanAes) -> [u32; 4] {
    std::array::from_fn(|index| aes.read(None, 0x64 + 4 * index as u64) as u32)
}

fn set_key(aes: &mut OpenTitanAes) {
    let share0 = [
        0x3c2d1e0f, 0x78695a4b, 0xb4a59687, 0xf0e1d2c3, 0x29380b1a, 0x6d7c4f5e, 0xa1b08392,
        0xe5f4c7d6,
    ];
    let share1 = [
        0x3f2f1f0f, 0x7f6f5f4f, 0xbfaf9f8f, 0xffefdfcf, 0x3a2a1a0a, 0x7a6a5a4a, 0xbaaa9a8a,
        0xfaeadaca,
    ];
    for (index, (first, second)) in share0.into_iter().zip(share1).enumerate() {
        write(aes, 0x04 + 4 * index as u64, first);
        write(aes, 0x24 + 4 * index as u64, second);
    }
}

#[test]
fn should_start_in_idle_mode() {
    assert_eq!(setup().read(None, STATUS) & 1, 1);
}

#[test]
fn should_change_status_when_set_to_automatic_mode() {
    let mut aes = setup();
    write(&mut aes, CONTROL, 0x8);
    assert_eq!(aes.read(None, STATUS) & 0x10, 0x10);
}

#[test]
fn should_keep_track_of_data_writes() {
    let mut aes = setup();
    write(&mut aes, 0x54, 0x1234);
    assert!(!aes.input_data_ready());
    input(&mut aes, ENCRYPTED);
    assert!(aes.input_data_ready());
}

#[test]
fn should_give_correct_encryption_output() {
    let mut aes = setup();
    set_key(&mut aes);
    write(&mut aes, CONTROL, 0x205);
    input(&mut aes, DECRYPTED);
    assert_eq!(output(&mut aes), ENCRYPTED);
}

#[test]
fn should_work_in_manual_mode_too() {
    let mut aes = setup();
    set_key(&mut aes);
    write(&mut aes, CONTROL, 0x8405);
    input(&mut aes, DECRYPTED);
    assert_eq!(output(&mut aes), [0; 4]);
    write(&mut aes, TRIGGER, 1);
    assert_eq!(output(&mut aes), ENCRYPTED);
}

#[test]
fn should_give_correct_decryption_output() {
    let mut aes = setup();
    set_key(&mut aes);
    write(&mut aes, CONTROL, 0x206);
    input(&mut aes, ENCRYPTED);
    assert_eq!(output(&mut aes), DECRYPTED);
}

#[test]
fn should_randomize_data_output_on_trigger() {
    let mut aes = setup();
    write(&mut aes, TRIGGER, 4);
    assert_ne!(output(&mut aes).into_iter().fold(0, u32::wrapping_add), 0);
}

#[test]
fn should_randomize_input_registers_on_trigger() {
    let mut aes = setup();
    write(&mut aes, CONTROL, 0x1005);
    input(&mut aes, DECRYPTED);
    write(&mut aes, TRIGGER, 6);
    write(&mut aes, TRIGGER, 1);
    for (actual, encrypted) in output(&mut aes).into_iter().zip(ENCRYPTED) {
        assert_ne!(actual, encrypted);
    }
}

#[test]
fn should_be_able_to_do_multiple_transformations_with_a_single_key() {
    let mut aes = setup();
    set_key(&mut aes);
    write(&mut aes, CONTROL, 0x206);
    input(&mut aes, DECRYPTED);
    input(&mut aes, ENCRYPTED);
    assert_eq!(output(&mut aes), DECRYPTED);
}

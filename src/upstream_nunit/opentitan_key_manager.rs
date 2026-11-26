use crate::{
    machine::Machine,
    mapped_memory::MappedMemory,
    opentitan_key_manager::{
        ERROR_CODE, KeyManager, KeyMaterial, OPERATION_CONTROLS, Operation, STATUS, WORKING_STATE,
        WorkingState,
    },
};
use Operation::*;
use WorkingState::*;

fn hex<const N: usize>(input: &str) -> [u8; N] {
    assert_eq!(input.len(), N * 2);
    std::array::from_fn(|i| u8::from_str_radix(&input[i * 2..i * 2 + 2], 16).unwrap())
}

fn setup() -> KeyManager {
    let _machine = Machine::without_default_ram();
    let _rom = MappedMemory::new(0x100, 0x100, |_| {}).unwrap();
    // The upstream controller is constructed with this nonce and key and never loads a ROM image.
    // Its digest is zero-initialized and Reset checks it without hashing the mapped memory.
    let _nonce: [u8; 8] = hex("0000000000000000");
    let _key: [u8; 8] = hex("0000000000000000");
    let rom_digest = [0; 32];
    KeyManager::new(
        KeyMaterial {
            device_id: hex("fa53b8058e157cb69f1f413e87242971b6b52a656a1cab7febf21e5bf1f45edd"),
            life_cycle_diversification_constant: hex("6faf88f22bccd612d1c09f5c02b2c8d1"),
            creator_key: hex("9152e32c9380a4bcc3e0ab263581e6b0e8825186e1e445631646e8bef8c45d47"),
            owner_key: hex("fa365df52da48cd752fb3a026a8e608f0098cfe5fa9810494829d0cd9479eb78"),
            root_key: hex(
                "efb7ea7ee90093cf4affd9aaa2d6c0ec446cfdf5f2d5a0bfd7e2d93edc63a10256d24a00181de99e0f690b447a8dde2a1ffb8bc306707107aa6e2410f15cfc37",
            ),
            soft_output_seed: hex(
                "df273097a573a411332efd86009bd0a175f08814ecc17ab02cc1e3404e1cd8bf",
            ),
            hard_output_seed: hex(
                "69582e71443c8be0fc00de9d9734c3fe7f4266d10a752de74814f2a3079f69a3",
            ),
            revision_seed: hex("17a9838dd4cd7f1bdce673b937a6d75202fedbf893bf7d52c8a744ad83d2630b"),
            destination_seeds: [
                hex("73e5bc251b143b74476e576754125d61930d203f199a87c123c074e020fd5028"),
                hex("ce44cbff5e09e6dd3ae54e9e45da6e662fb69c3aab936b415a0d6e7185eaa2e0"),
                hex("b76a8aff9e4da0e3ff9f3036fd9c13ac08496db56fbc4894d38bd8674f4b542d"),
                hex("fcc581b66ae11d33f678e7d227881bcfe58a331208f189de6265edc8fde06db0"),
            ],
            identity_seeds: [
                hex("c20c05a20251023541544776930be76bfbb22e1d8aaa4783f2b5e094e3e8d3f8"),
                hex("93cdb1d9a6a60050ef0d8a166d91200dc6757907237df4401908799dfa1fe8f2"),
                hex("a88601ca1695a7c8c5d32486aac4e086628d6c8ca138f65d25dfa5f9c912f354"),
            ],
        },
        rom_digest,
    )
}

fn command(key_manager: &mut KeyManager, command: Operation) {
    key_manager.write_double_word(OPERATION_CONTROLS, ((command as u32) << 4) | 1);
}

fn assert_error(key_manager: &mut KeyManager, expected: bool, message: &str) {
    let error = key_manager.read_double_word(ERROR_CODE);
    let status = key_manager.read_double_word(STATUS);
    if expected {
        assert_ne!(error, 0, "{message}");
        assert_eq!(status, 3, "{message}");
    } else {
        assert_eq!(error, 0, "{message}");
        assert_ne!(status, 3, "{message}");
    }
}

#[test]
fn init_to_reset_state() {
    let mut peripheral = setup();
    assert_eq!(peripheral.read_double_word(WORKING_STATE), Reset as u32);
}

#[test]
fn advance_state_properly() {
    let mut peripheral = setup();
    for expected in [
        Init,
        CreatorRootKey,
        OwnerIntermediateKey,
        OwnerKey,
        Disabled,
        Disabled,
    ] {
        command(&mut peripheral, Advance);
        assert_eq!(peripheral.read_double_word(WORKING_STATE), expected as u32);
    }
}

#[test]
fn ignore_other_commands_in_reset_state() {
    let mut peripheral = setup();
    peripheral.reset();
    command(&mut peripheral, Disable);
    assert_error(&mut peripheral, false, "");
    command(&mut peripheral, GenerateId);
    assert_error(&mut peripheral, false, "");
}

#[test]
fn clear_errors_after_valid_operation() {
    let mut peripheral = setup();
    command(&mut peripheral, Advance);
    command(&mut peripheral, GenerateId);
    assert_error(&mut peripheral, true, "GenerateID in Init state");
    peripheral.write_double_word(ERROR_CODE, 7);
    command(&mut peripheral, Advance);
    assert_error(&mut peripheral, false, "Advance in Init state");
}

#[test]
fn raise_error_on_illegal_command() {
    let mut peripheral = setup();
    peripheral.reset();
    command(&mut peripheral, Advance);
    for (op, name) in [
        (GenerateId, "GenerateID"),
        (GenerateSwOutput, "GenerateSWOutput"),
        (GenerateHwOutput, "GenerateHWOutput"),
    ] {
        command(&mut peripheral, op);
        assert_error(&mut peripheral, true, &format!("{name} in Init state"));
    }
    for _ in 0..4 {
        command(&mut peripheral, Advance);
    }
    for (op, name) in [
        (Advance, "Advance"),
        (Disable, "Disable"),
        (GenerateId, "GenerateID"),
        (GenerateSwOutput, "GenerateSWOutput"),
        (GenerateHwOutput, "GenerateHWOutput"),
    ] {
        command(&mut peripheral, op);
        assert_error(&mut peripheral, true, &format!("{name} in Disabled state"));
    }
}

use crate::cadence_uart::CadenceUart;

const CONTROL: u64 = 0;
const STATUS: u64 = 0x14;
const ENABLE_RX: u32 = 1 << 2;
const TX_EMPTY: u32 = 1 << 3;
const RX_OVERFLOW: u32 = 1 << 5;
const INITIAL: u32 = TX_EMPTY | (1 << 1);
const AFTER_WRITE: u32 = INITIAL | (1 << 8);
const CAPACITY: usize = 64;

fn setup(clear_on_read: bool, capacity: usize) -> CadenceUart {
    let mut uart = CadenceUart::new(clear_on_read, capacity);
    uart.reset();
    uart
}

#[test]
fn should_clear_interrupt_status_on_write_one() {
    let mut uart = setup(false, CAPACITY);
    uart.write_register(CONTROL, ENABLE_RX);
    assert_eq!(uart.read_register(STATUS), INITIAL);
    uart.write_char(0);
    assert_eq!(uart.read_register(STATUS), AFTER_WRITE);
    assert_eq!(uart.read_register(STATUS), AFTER_WRITE);
    uart.write_register(STATUS, AFTER_WRITE);
    assert_eq!(uart.read_register(STATUS), TX_EMPTY);
}

#[test]
fn should_clear_interrupt_status_on_read() {
    let mut uart = setup(true, CAPACITY);
    uart.write_register(CONTROL, ENABLE_RX);
    assert_eq!(uart.read_register(STATUS), INITIAL);
    uart.write_char(0);
    assert_eq!(uart.read_register(STATUS), AFTER_WRITE);
    assert_eq!(uart.read_register(STATUS), TX_EMPTY);
}

fn assert_tx_flag(previous_bytes: usize, flag: u32) {
    let mut uart = setup(true, CAPACITY);
    for _ in 0..previous_bytes {
        uart.transmit_character(0);
    }
    assert_eq!(uart.read_register(STATUS) & flag, 0);
    uart.transmit_character(0);
    assert_ne!(uart.read_register(STATUS) & flag, 0);
}

#[test]
fn should_generate_tx_almost_full_interrupt_when_tx_disabled() {
    assert_tx_flag(CAPACITY - 2, 1 << 11);
}

#[test]
fn should_generate_tx_full_interrupt_when_tx_disabled() {
    assert_tx_flag(CAPACITY - 1, 1 << 4);
}

#[test]
fn should_generate_tx_overflow_interrupt_when_tx_disabled() {
    assert_tx_flag(CAPACITY, 1 << 12);
}

#[test]
fn should_generate_rx_overflows_when_enabled() {
    let mut uart = setup(true, 1);
    uart.enable_rx_overflow = true;
    uart.write_register(CONTROL, ENABLE_RX);
    uart.write_char(0);
    assert_eq!(uart.read_register(STATUS) & RX_OVERFLOW, 0);
    uart.write_char(0);
    assert_ne!(uart.read_register(STATUS) & RX_OVERFLOW, 0);

    uart.reset();
    uart.enable_rx_overflow = false;
    uart.write_register(CONTROL, ENABLE_RX);
    uart.write_char(0);
    assert_eq!(uart.read_register(STATUS) & RX_OVERFLOW, 0);
    uart.write_char(0);
    assert_eq!(uart.read_register(STATUS) & RX_OVERFLOW, 0);
}

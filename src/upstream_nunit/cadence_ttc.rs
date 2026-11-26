use std::time::Duration;

use crate::cadence_ttc::CadenceTtc;

const CLOCK_CONTROL: u64 = 0x00;
const COUNTER_CONTROL: u64 = 0x0c;
const COUNTER_VALUE: u64 = 0x18;
const COUNTER_INTERVAL: u64 = 0x24;
const INTERRUPT_STATUS: u64 = 0x54;
const INTERRUPT_ENABLE: u64 = 0x60;
const TICKS_PER_SECOND: u32 = 0x100;
const PRESCALER_1024: u32 = (9 << 1) | 1;

fn setup() -> CadenceTtc {
    let mut timer = CadenceTtc::new(u64::from(TICKS_PER_SECOND * 1024));
    timer.reset();
    timer
}

#[test]
fn should_reset_counter() {
    let mut timer = setup();
    timer.set_counter_value(0, TICKS_PER_SECOND);
    timer.write_register(INTERRUPT_ENABLE, 0xff);
    assert_eq!(timer.read_register(COUNTER_VALUE), TICKS_PER_SECOND);
    assert!(!timer.irq(0));

    timer.write_register(COUNTER_CONTROL, 0x10);
    assert_eq!(timer.read_register(COUNTER_VALUE), 0);
    assert!(!timer.irq(0));
}

#[test]
fn should_count_independently() {
    let mut timer = setup();
    timer.write_register(CLOCK_CONTROL, PRESCALER_1024);
    timer.write_register(CLOCK_CONTROL + 4, (10 << 1) | 1);
    timer.write_register(CLOCK_CONTROL + 8, (11 << 1) | 1);
    for offset in [0, 4, 8] {
        timer.write_register(COUNTER_CONTROL + offset, 0);
        assert_eq!(timer.read_register(COUNTER_VALUE + offset), 0);
    }
    timer.advance(Duration::from_secs(1));
    assert_eq!(timer.read_register(COUNTER_VALUE), TICKS_PER_SECOND);
    assert_eq!(timer.read_register(COUNTER_VALUE + 4), TICKS_PER_SECOND / 2);
    assert_eq!(timer.read_register(COUNTER_VALUE + 8), TICKS_PER_SECOND / 4);
}

fn should_overflow_in_one_second(
    control: u32,
    mask: u32,
    initial: u32,
    overflow: u32,
    interval: u32,
) {
    let mut timer = setup();
    timer.write_register(CLOCK_CONTROL, PRESCALER_1024);
    timer.write_register(COUNTER_CONTROL, control);
    timer.write_register(COUNTER_INTERVAL, interval);
    timer.write_register(INTERRUPT_ENABLE, mask);
    timer.read_register(INTERRUPT_STATUS);

    timer.set_counter_value(0, initial);
    timer.advance(Duration::from_secs(1));
    assert!(!timer.irq(0));

    timer.advance(Duration::from_secs_f64(1.0 / f64::from(TICKS_PER_SECOND)));
    assert_eq!(timer.read_register(COUNTER_VALUE), overflow);
    assert!(timer.irq(0));
    assert_eq!(timer.read_register(INTERRUPT_STATUS), mask);
    assert!(!timer.irq(0));
}

#[test]
fn should_count_up_in_overflow_mode() {
    should_overflow_in_one_second(0, 0x10, u32::MAX - TICKS_PER_SECOND - 1, 0, 0);
}

#[test]
fn should_count_down_in_overflow_mode() {
    should_overflow_in_one_second(4, 0x10, TICKS_PER_SECOND + 1, u32::MAX, 0);
}

#[test]
fn should_count_up_in_interval_mode() {
    should_overflow_in_one_second(2, 1, 2 * TICKS_PER_SECOND - 1, 0, 3 * TICKS_PER_SECOND);
}

#[test]
fn should_count_down_in_interval_mode() {
    should_overflow_in_one_second(
        6,
        1,
        TICKS_PER_SECOND + 1,
        3 * TICKS_PER_SECOND,
        3 * TICKS_PER_SECOND,
    );
}

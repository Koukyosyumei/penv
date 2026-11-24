mod bit_helper;
mod circular_buffer;
mod dma_engine;
mod misc;
mod parser_primitives;
mod range;
mod system_bus;
mod time_interval;

use crate::{parse_address64, repl_syntax};

#[test]
fn quoted_string() {
    assert_eq!(
        repl_syntax::quoted_string(r#""some text""#).as_deref(),
        Some("some text")
    );
}

#[test]
fn quoted_string_with_semicolon() {
    assert_eq!(
        repl_syntax::quoted_string(r#""some;text""#).as_deref(),
        Some("some;text")
    );
}

#[test]
fn hexadecimal_unsigned_long_1000() {
    assert_eq!(parse_address64("0x1000"), Ok(0x1000));
}

#[test]
fn hexadecimal_unsigned_long_12ab() {
    assert_eq!(parse_address64("0x12AB"), Ok(0x12ab));
}

#[test]
fn hexadecimal_unsigned_long_0ff() {
    assert_eq!(parse_address64("0x0ff"), Ok(0xff));
}

mod memory;

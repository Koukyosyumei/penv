mod access_condition;
mod bit_helper;
mod circular_buffer;
mod data_storage;
mod dma_engine;
mod misc;
mod parser_primitives;
mod read_extensions;
mod range;
mod storage;
mod symbol;
mod symbol_deduplication;
mod symbol_lookup;
mod elf_merge;
mod simple_cache;
mod serializable_mapped_segment;
mod pseudorandom;
mod sht21;
mod bma180;
mod efm32_i2c;
mod efr32_i2c;
mod lpc43xx_gpio;
mod cadence_ttc;
mod cadence_uart;
mod bmc050;
mod register_enum;
mod register_fields;
mod svd;
mod opentitan_aes;
mod opentitan_i2c;
mod python_monitor;
mod packet_layout;
mod monitor_tokenizer;
mod machine_registry;
mod multi_cpu;
mod migrant_blocking_collection;
mod weak_multi_table;
mod option_validation;
mod option_parser;
mod system_bus;
mod peripherals_group;
mod null_registration;
mod time_interval;
mod time_source;
mod clock_source;
mod comparing_timer;
mod tracked_register_bytes;
mod write_extensions;

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
mod migrant;
mod migrant_descriptor;
mod migrant_stamping;
mod migrant_surrogate;
mod migrant_builtin;
mod migrant_hooks;
mod multi_tree;
mod multi_value_dictionary;
mod migrant_object_stream;
mod migrant_user_lists;

mod type_extensions_cache;

mod monitor_features;

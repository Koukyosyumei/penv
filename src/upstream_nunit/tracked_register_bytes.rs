use crate::tracked_register_bytes::{Error, TrackedRegisterBytes};

fn fixture() -> TrackedRegisterBytes {
    TrackedRegisterBytes::new(4, 4).unwrap()
}

fn write_words(array: &mut TrackedRegisterBytes, words: &[u32]) {
    for (part, word) in words.iter().copied().enumerate() {
        array.set_part(part, word).unwrap();
    }
}

#[test]
fn should_allow_only_access_with_width_equal4() {
    let _array = fixture();
    for invalid in [3, 2, 1] {
        assert!(matches!(
            TrackedRegisterBytes::new(4, invalid),
            Err(Error::UnsupportedWidth)
        ));
    }
}

#[test]
fn handle_accessing_input_data_through_registers() {
    let mut array = fixture();
    write_words(
        &mut array,
        &[0x88442211, 0x80402010, 0x18244281, 0x08040201],
    );
    assert_eq!(
        array.retrieve(true),
        &[
            0x11, 0x22, 0x44, 0x88, 0x10, 0x20, 0x40, 0x80, 0x81, 0x42, 0x24, 0x18, 1, 2, 4, 8
        ]
    );
    write_words(&mut array, &[0x88442211, 0, 0, 0x18244281]);
    assert_eq!(
        array.retrieve(true),
        &[
            0x11, 0x22, 0x44, 0x88, 0, 0, 0, 0, 0, 0, 0, 0, 0x81, 0x42, 0x24, 0x18
        ]
    );
}

#[test]
fn should_know_if_not_all_parts_written() {
    let mut array = fixture();
    assert!(!array.all_written());
    for index in 0..3 {
        array.set_part(index, 0).unwrap();
        assert!(!array.all_written());
    }
}

#[test]
fn should_not_retrive_data_until_all_parts_written() {
    let mut array = fixture();
    write_words(&mut array, &[0; 3]);
    assert!(array.retrieve(true).is_empty());
}

#[test]
fn should_know_if_all_parts_written() {
    let mut array = fixture();
    write_words(&mut array, &[0; 4]);
    assert!(array.all_written());
}

#[test]
fn should_retrive_data_when_all_written() {
    let mut array = fixture();
    write_words(&mut array, &[0; 4]);
    assert!(!array.retrieve(true).is_empty());
}

#[test]
fn should_be_able_to_preserve_tracking_when_retriving_data() {
    let mut array = fixture();
    write_words(&mut array, &[0; 4]);
    array.retrieve(false);
    assert!(array.all_written());
}

#[test]
fn should_clear_access_tracking_after_getting_new_data() {
    let mut array = fixture();
    write_words(&mut array, &[0; 4]);
    array.set_part(3, 0).unwrap();
    assert!(!array.all_written());
}

#[test]
fn handle_accessing_output_data_through_registers() {
    let mut array = fixture();
    array
        .set_array(
            vec![
                1, 2, 4, 8, 0x11, 0x22, 0x44, 0x88, 0x10, 0x20, 0x40, 0x80, 0x81, 0x42, 0x24, 0x18,
            ],
            true,
        )
        .unwrap();
    for (part, expected) in [0x08040201, 0x88442211, 0x80402010, 0x18244281]
        .into_iter()
        .enumerate()
    {
        assert_eq!(array.part(part).unwrap(), expected);
    }
}

#[test]
fn should_throw_when_setting_array_with_wrong_length() {
    assert_eq!(
        fixture().set_array(vec![0; 6], true),
        Err(Error::IncorrectLength)
    );
}

#[test]
fn should_throw_when_trying_to_get_unexisting_part() {
    assert_eq!(fixture().set_part(17, 0), Err(Error::PartOutOfRange));
}

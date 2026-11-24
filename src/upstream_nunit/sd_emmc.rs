use crate::sd_emmc::Card;

fn write_at(card: &mut Card, sector: u32, data: &[u8]) {
    card.handle_command(24, sector).unwrap();
    card.write_data(data).unwrap();
}

fn read_at(card: &mut Card, sector: u32, length: usize) -> Vec<u8> {
    card.handle_command(17, sector).unwrap();
    card.read_data(length).unwrap()
}

#[test]
fn should_reject_spi_mode() {
    let error = Card::new(0x1000000, true)
        .err()
        .expect("expected constructor failure");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
}

#[test]
fn should_use_sector_addressing_for_emmc() {
    let marker = [0xAB, 0xCD, 0xEF, 0x01];
    let mut card = Card::new(0x1000000, false).unwrap();
    card.reset();
    write_at(&mut card, 2, &marker);
    assert_eq!(marker, read_at(&mut card, 2, marker.len()).as_slice());
    assert_ne!(marker, read_at(&mut card, 0, marker.len()).as_slice());
}

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

fn switch_partition(card: &mut Card, access: u32) -> u32 {
    card.handle_command(6, (3 << 24) | (179 << 16) | (access << 8))
        .unwrap()
}

#[test]
fn should_keep_hardware_partitions_separate() {
    const SECTOR: u32 = 0x1000 / 512;
    let uda = [0x55, 0x44, 0x41, 0x30];
    let boot = [0x42, 0x4F, 0x4F, 0x54];
    let mut card = Card::new(0x1000000, false).unwrap();
    card.reset();

    write_at(&mut card, SECTOR, &uda);
    switch_partition(&mut card, 1);
    write_at(&mut card, SECTOR, &boot);

    switch_partition(&mut card, 0);
    switch_partition(&mut card, 1);
    assert_eq!(boot, read_at(&mut card, SECTOR, boot.len()).as_slice());

    switch_partition(&mut card, 0);
    assert_eq!(uda, read_at(&mut card, SECTOR, uda.len()).as_slice());
}

#[test]
fn should_reset_partition_and_extended_csd_transfer_on_cmd0() {
    const SECTOR: u32 = 0x1000 / 512;
    let uda = [0x55, 0x44, 0x41, 0x30];
    let boot = [0x42, 0x4F, 0x4F, 0x54];
    let mut card = Card::new(0x1000000, false).unwrap();
    card.reset();
    write_at(&mut card, SECTOR, &uda);
    switch_partition(&mut card, 1);
    write_at(&mut card, SECTOR, &boot);

    card.handle_command(8, 0).unwrap();
    card.read_data(1).unwrap();
    card.handle_command(0, 0).unwrap();

    assert_eq!(uda, read_at(&mut card, SECTOR, uda.len()).as_slice());
}

#[test]
fn should_reject_unavailable_general_purpose_partition() {
    let mut card = Card::new(0x1000000, false).unwrap();
    card.reset();

    let response = switch_partition(&mut card, 4);
    assert_ne!(response & (1 << 7), 0, "SWITCH_ERROR should be set");
    card.handle_command(8, 0).unwrap();
    assert_eq!(card.read_data(512).unwrap()[179] & 7, 0);
}

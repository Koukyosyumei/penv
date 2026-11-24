use super::primitive::{Reader, Writer};
use std::io::{self, Cursor};

/// Encode the contents written by a custom serializer into a bounded payload.
pub fn save(
    write: impl FnOnce(&mut Writer<&mut Vec<u8>>) -> io::Result<()>,
) -> io::Result<Vec<u8>> {
    let mut payload = Vec::new();
    let mut writer = Writer::new(&mut payload, false);
    write(&mut writer)?;
    writer.finish()?;
    Ok(payload)
}

/// Reject a custom reader that leaves data behind or reads past its own payload.
pub fn load<T>(
    payload: &[u8],
    read: impl FnOnce(&mut Reader<&mut Cursor<&[u8]>>) -> io::Result<T>,
) -> io::Result<T> {
    let mut cursor = Cursor::new(payload);
    let mut reader = Reader::new(&mut cursor, false);
    let result = read(&mut reader)?;
    reader.finish()?;
    if cursor.position() != payload.len() as u64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "custom deserialization did not consume its payload",
        ));
    }
    Ok(result)
}

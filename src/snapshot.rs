use std::io::{self, Read, Write};

pub(crate) const CORTEX_MAGIC: &[u8; 9] = b"PENVTHMB\x01";
pub(crate) const ARM32_MAGIC: &[u8; 9] = b"PENVARM3\x07";
pub(crate) const RV64_MAGIC: &[u8; 9] = b"PENVRV64\x01";

pub(crate) fn arm32_version(header: &[u8]) -> Option<u8> {
    let &[version @ 1..=7, ..] = header.strip_prefix(b"PENVARM3")? else { return None };
    Some(version)
}

pub(crate) fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

pub(crate) fn write_words(output: &mut impl Write, words: &[u64]) -> io::Result<()> {
    for word in words {
        output.write_all(&word.to_le_bytes())?;
    }
    Ok(())
}

pub(crate) fn read_words<const N: usize>(input: &mut impl Read) -> io::Result<[u64; N]> {
    let mut words = [0; N];
    for word in &mut words {
        let mut bytes = [0; 8];
        input.read_exact(&mut bytes)?;
        *word = u64::from_le_bytes(bytes);
    }
    Ok(words)
}

pub(crate) fn read_bool(value: u64) -> io::Result<bool> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(invalid("invalid snapshot boolean")),
    }
}

pub(crate) fn write_text(output: &mut impl Write, text: &str) -> io::Result<()> {
    if text.len() > 64 * 1024 {
        return Err(invalid("snapshot text exceeds size limit"));
    }
    write_words(output, &[text.len() as u64])?;
    output.write_all(text.as_bytes())
}

pub(crate) fn read_text(input: &mut impl Read) -> io::Result<String> {
    let [size] = read_words(input)?;
    if size > 64 * 1024 {
        return Err(invalid("snapshot text exceeds size limit"));
    }
    let mut data = vec![0; size as usize];
    input.read_exact(&mut data)?;
    String::from_utf8(data).map_err(io::Error::other)
}

//! Editing a v17 flattened device tree before placing it in guest RAM.
use std::ops::Range;

const INVALID: &str = "invalid flattened device tree";

fn word(bytes: &[u8], at: usize) -> Result<u32, &'static str> {
    let end = at.checked_add(4).ok_or(INVALID)?;
    Ok(u32::from_be_bytes(
        bytes.get(at..end).ok_or(INVALID)?.try_into().unwrap(),
    ))
}

fn block(bytes: &[u8], offset: u32, size: u32) -> Result<&[u8], &'static str> {
    let end = offset.checked_add(size).ok_or(INVALID)?;
    bytes.get(offset as usize..end as usize).ok_or(INVALID)
}

fn c_string(bytes: &[u8]) -> Result<&[u8], &'static str> {
    bytes
        .iter()
        .position(|&byte| byte == 0)
        .map(|end| &bytes[..end])
        .ok_or(INVALID)
}

fn put_word(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_be_bytes());
}

pub(crate) fn with_bootargs(
    blob: &[u8],
    args: &str,
    append: bool,
) -> Result<Vec<u8>, &'static str> {
    if word(blob, 0)? != 0xd00d_feed || word(blob, 20)? != 17 {
        return Err("LoadFdt with boot arguments requires a v17 flattened device tree");
    }
    let blob = block(blob, 0, word(blob, 4)?)?;
    let structure_offset = word(blob, 8)?;
    let strings_offset = word(blob, 12)?;
    let reserve_offset = word(blob, 16)?;
    let structure_size = word(blob, 36)?;
    if reserve_offset < 40
        || reserve_offset >= structure_offset
        || structure_offset
            .checked_add(structure_size)
            .ok_or(INVALID)?
            > strings_offset
    {
        return Err(INVALID);
    }
    let structure = block(blob, structure_offset, structure_size)?;
    let mut strings = block(blob, strings_offset, word(blob, 32)?)?.to_vec();

    let mut pos = 0;
    let mut depth = 0usize;
    let mut chosen_insert = None;
    let mut in_chosen = false;
    let mut root_end = None;
    let mut property: Option<(Range<usize>, &[u8], u32)> = None;
    loop {
        let start = pos;
        let token = word(structure, pos)?;
        pos += 4;
        match token {
            1 => {
                let name = c_string(&structure[pos..])?;
                pos = (pos + name.len() + 1).next_multiple_of(4);
                if depth == 1 && name == b"chosen" && chosen_insert.is_none() {
                    chosen_insert = Some(pos);
                    in_chosen = true;
                }
                depth += 1;
            }
            2 => {
                if depth == 2 {
                    in_chosen = false;
                }
                if depth == 1 {
                    root_end = Some(start);
                }
                depth = depth.checked_sub(1).ok_or(INVALID)?;
            }
            3 => {
                let size = word(structure, pos)?;
                let name_offset = word(structure, pos + 4)?;
                pos += 8;
                let value = block(structure, u32::try_from(pos).map_err(|_| INVALID)?, size)?;
                pos = (pos + value.len()).next_multiple_of(4);
                if in_chosen
                    && depth == 2
                    && property.is_none()
                    && c_string(strings.get(name_offset as usize..).ok_or(INVALID)?)? == b"bootargs"
                {
                    property = Some((start..pos, value, name_offset));
                }
            }
            4 => {}
            9 if depth == 0 => break,
            _ => return Err(INVALID),
        }
    }

    let mut updated = Vec::new();
    let (range, previous, name_offset) = if let Some(property) = property {
        property
    } else {
        let insertion = if let Some(insertion) = chosen_insert {
            insertion
        } else {
            put_word(&mut updated, 1);
            updated.extend_from_slice(b"chosen\0\0");
            root_end.ok_or(INVALID)?
        };
        let name_offset = u32::try_from(strings.len()).map_err(|_| INVALID)?;
        strings.extend_from_slice(b"bootargs\0");
        (insertion..insertion, &[][..], name_offset)
    };
    let previous = if append && !previous.is_empty() {
        previous.strip_suffix(&[0]).ok_or(INVALID)?
    } else {
        &[]
    };
    put_word(&mut updated, 3);
    let length = previous
        .len()
        .checked_add(args.len())
        .and_then(|len| len.checked_add(1))
        .ok_or(INVALID)?;
    put_word(&mut updated, u32::try_from(length).map_err(|_| INVALID)?);
    put_word(&mut updated, name_offset);
    updated.extend_from_slice(previous);
    updated.extend_from_slice(args.as_bytes());
    updated.push(0);
    updated.resize(updated.len().next_multiple_of(4), 0);
    if chosen_insert.is_none() {
        put_word(&mut updated, 2);
    }

    let mut result = blob[..structure_offset as usize].to_vec();
    result.extend_from_slice(structure.get(..range.start).ok_or(INVALID)?);
    result.extend_from_slice(&updated);
    result.extend_from_slice(structure.get(range.end..).ok_or(INVALID)?);
    let strings_offset = u32::try_from(result.len()).map_err(|_| INVALID)?;
    result.extend_from_slice(&strings);
    for (offset, value) in [
        (4, u32::try_from(result.len()).map_err(|_| INVALID)?),
        (12, strings_offset),
        (32, u32::try_from(strings.len()).map_err(|_| INVALID)?),
        (36, strings_offset - structure_offset),
    ] {
        result[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
    }
    Ok(result)
}

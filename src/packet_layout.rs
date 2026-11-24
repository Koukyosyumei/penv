#[derive(Clone, Copy)]
pub enum ByteOrder {
    Little,
    Big,
}

#[derive(Clone, Copy)]
pub struct Field {
    pub offset: Option<usize>,
    pub kind: FieldKind,
}

#[derive(Clone, Copy)]
pub enum FieldKind {
    Scalar { width: usize, capacity: usize },
    Bytes(usize),
    Nested(&'static Layout),
}

pub struct Layout {
    pub order: ByteOrder,
    pub fields: &'static [Field],
}

#[derive(Debug, PartialEq, Eq)]
pub enum Decoded {
    Scalar(u64),
    Bytes(Vec<u8>),
    Nested(Vec<Decoded>),
}

#[derive(Debug, PartialEq, Eq)]
pub enum DecodeError {
    InvalidWidth,
    Unaligned,
    InsufficientData,
    LayoutOverflow,
}

impl Layout {
    fn visit(
        &self,
        mut f: impl FnMut(usize, usize, FieldKind) -> Result<(), DecodeError>,
    ) -> Result<usize, DecodeError> {
        let mut cursor = 0;
        let mut size = 0;
        for field in self.fields {
            let offset = field.offset.unwrap_or(cursor);
            let width = match field.kind {
                FieldKind::Scalar { width, capacity } if width <= capacity && capacity <= 64 => {
                    width
                }
                FieldKind::Scalar { .. } => return Err(DecodeError::InvalidWidth),
                FieldKind::Bytes(bytes) => {
                    bytes.checked_mul(8).ok_or(DecodeError::LayoutOverflow)?
                }
                FieldKind::Nested(layout) => layout
                    .byte_len()?
                    .checked_mul(8)
                    .ok_or(DecodeError::LayoutOverflow)?,
            };
            cursor = offset
                .checked_add(width)
                .ok_or(DecodeError::LayoutOverflow)?;
            size = size.max(cursor);
            f(offset, width, field.kind)?;
        }
        Ok(size.div_ceil(8))
    }

    pub fn byte_len(&self) -> Result<usize, DecodeError> {
        self.visit(|_, _, _| Ok(()))
    }

    pub fn decode(&self, data: &[u8]) -> Result<Vec<Decoded>, DecodeError> {
        if data.len() < self.byte_len()? {
            return Err(DecodeError::InsufficientData);
        }
        let mut result = Vec::new();
        self.visit(|offset, width, kind| {
            let value = match kind {
                FieldKind::Scalar { .. } => {
                    let mut value = 0;
                    // Each group is bounded by a physical byte. Little endian
                    // consumes the first group as the least significant bits.
                    let mut consumed = 0;
                    while consumed < width {
                        let bit = offset + consumed;
                        let first = bit % 8;
                        let bits = (width - consumed).min(8 - first);
                        let part = (u64::from(data[bit / 8]) >> first) & ((1 << bits) - 1);
                        match self.order {
                            ByteOrder::Little => value |= part << consumed,
                            ByteOrder::Big => value = (value << bits) | part,
                        }
                        consumed += bits;
                    }
                    Decoded::Scalar(value)
                }
                FieldKind::Bytes(_) | FieldKind::Nested(_) if offset % 8 != 0 => {
                    return Err(DecodeError::Unaligned);
                }
                FieldKind::Bytes(bytes) => {
                    Decoded::Bytes(data[offset / 8..offset / 8 + bytes].to_vec())
                }
                FieldKind::Nested(layout) => Decoded::Nested(layout.decode(&data[offset / 8..])?),
            };
            result.push(value);
            Ok(())
        })?;
        Ok(result)
    }
}

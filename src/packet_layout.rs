#[derive(Clone, Copy)]
pub enum ByteOrder {
    Little,
    Big,
}

#[derive(Clone, Copy)]
pub struct Field {
    pub offset: Option<usize>,
    pub kind: FieldKind,
    pub padding: usize,
    pub alignment: usize,
}

#[derive(Clone, Copy)]
pub enum FieldKind {
    Scalar {
        width: usize,
        capacity: usize,
    },
    Bytes(usize),
    Nested(&'static Layout),
    Elements {
        count: usize,
        element_bits: Option<usize>,
        explicit_bits: Option<usize>,
    },
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
    Elements(Vec<u64>),
    NullBytes,
}

#[derive(Debug, PartialEq, Eq)]
pub enum DecodeError {
    InvalidWidth,
    Unaligned,
    InsufficientData,
    LayoutOverflow,
    InvalidOffset,
    InvalidValue,
}

impl Layout {
    fn visit(
        &self,
        mut f: impl FnMut(usize, usize, FieldKind) -> Result<(), DecodeError>,
    ) -> Result<usize, DecodeError> {
        let mut cursor = 0;
        let mut size = 0;
        for field in self.fields {
            let mut offset = field
                .offset
                .unwrap_or(cursor)
                .checked_add(field.padding)
                .ok_or(DecodeError::LayoutOverflow)?;
            if field.alignment > 0 {
                let extra = (field.alignment - offset % field.alignment) % field.alignment;
                offset = offset
                    .checked_add(extra)
                    .ok_or(DecodeError::LayoutOverflow)?;
            }
            let width = match field.kind {
                FieldKind::Scalar { width, capacity } if width <= capacity && capacity <= 64 => {
                    width
                }
                FieldKind::Scalar { .. } => return Err(DecodeError::InvalidWidth),
                FieldKind::Bytes(bytes) => {
                    bytes.checked_mul(8).ok_or(DecodeError::LayoutOverflow)?
                }
                FieldKind::Elements {
                    count,
                    element_bits,
                    explicit_bits,
                } => {
                    let bits = element_bits
                        .filter(|bits| matches!(bits, 8 | 16 | 32 | 64))
                        .ok_or(DecodeError::InvalidWidth)?;
                    let width = count.checked_mul(bits).ok_or(DecodeError::LayoutOverflow)?;
                    if explicit_bits.is_some_and(|explicit| explicit != width) {
                        return Err(DecodeError::InvalidWidth);
                    }
                    width
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

    pub fn decode_at(&self, data: &[u8], offset: isize) -> Result<Vec<Decoded>, DecodeError> {
        let offset = usize::try_from(offset).map_err(|_| DecodeError::InvalidOffset)?;
        self.decode(data.get(offset..).ok_or(DecodeError::InsufficientData)?)
    }

    fn scalar_groups(
        &self,
        offset: usize,
        width: usize,
        mut group: impl FnMut(usize, usize, usize, usize),
    ) {
        let mut consumed = 0;
        while consumed < width {
            let bit = offset + consumed;
            let first = bit % 8;
            let bits = (width - consumed).min(8 - first);
            let shift = match self.order {
                ByteOrder::Little => consumed,
                ByteOrder::Big => width - consumed - bits,
            };
            group(bit / 8, first, bits, shift);
            consumed += bits;
        }
    }

    fn read_scalar(&self, data: &[u8], offset: usize, width: usize) -> u64 {
        let mut value = 0;
        self.scalar_groups(offset, width, |byte, first, bits, shift| {
            value |= ((u64::from(data[byte]) >> first) & ((1 << bits) - 1)) << shift;
        });
        value
    }

    fn write_scalar(&self, data: &mut [u8], offset: usize, width: usize, value: u64) {
        self.scalar_groups(offset, width, |byte, first, bits, shift| {
            let mask = ((1u16 << bits) - 1) as u8;
            data[byte] =
                (data[byte] & !(mask << first)) | (((value >> shift) as u8 & mask) << first);
        });
    }

    pub fn encode(&self, values: &[Decoded]) -> Result<Vec<u8>, DecodeError> {
        if values.len() != self.fields.len() {
            return Err(DecodeError::InvalidValue);
        }
        let mut data = vec![0; self.byte_len()?];
        let mut values = values.iter();
        self.visit(|offset, width, kind| {
            let value = values.next().unwrap();
            match (kind, value) {
                (FieldKind::Scalar { .. }, Decoded::Scalar(value)) => {
                    if width == 0 && offset != 0 && offset / 8 >= data.len() {
                        return Err(DecodeError::InvalidOffset);
                    }
                    self.write_scalar(&mut data, offset, width, *value);
                }
                (_, _) if offset % 8 != 0 => return Err(DecodeError::Unaligned),
                (FieldKind::Bytes(count), Decoded::Bytes(bytes)) if bytes.len() == count => {
                    data[offset / 8..offset / 8 + count].copy_from_slice(bytes);
                }
                (FieldKind::Bytes(_), Decoded::NullBytes) => {}
                (FieldKind::Nested(layout), Decoded::Nested(fields)) => {
                    let bytes = layout.encode(fields)?;
                    data[offset / 8..offset / 8 + bytes.len()].copy_from_slice(&bytes);
                }
                (
                    FieldKind::Elements {
                        count,
                        element_bits: Some(bits),
                        ..
                    },
                    Decoded::Elements(values),
                ) if values.len() == count => {
                    for (index, value) in values.iter().enumerate() {
                        self.write_scalar(&mut data, offset + index * bits, bits, *value);
                    }
                }
                _ => return Err(DecodeError::InvalidValue),
            }
            Ok(())
        })?;
        Ok(data)
    }

    pub fn decode(&self, data: &[u8]) -> Result<Vec<Decoded>, DecodeError> {
        if data.len() < self.byte_len()? {
            return Err(DecodeError::InsufficientData);
        }
        let mut result = Vec::new();
        self.visit(|offset, width, kind| {
            let value = match kind {
                FieldKind::Scalar { .. } => Decoded::Scalar(self.read_scalar(data, offset, width)),
                _ if offset % 8 != 0 => return Err(DecodeError::Unaligned),
                FieldKind::Elements {
                    count,
                    element_bits,
                    ..
                } => {
                    let bits = element_bits.ok_or(DecodeError::InvalidWidth)?;
                    Decoded::Elements(
                        (0..count)
                            .map(|index| self.read_scalar(data, offset + index * bits, bits))
                            .collect(),
                    )
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

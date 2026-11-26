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

pub type Presence = fn(&[Decoded]) -> bool;

pub struct Layout {
    pub order: ByteOrder,
    pub conditions: &'static [(usize, Presence)],
    pub fields: &'static [Field],
}

#[derive(Debug, PartialEq, Eq)]
pub enum Decoded {
    Scalar(u64),
    Bytes(Vec<u8>),
    Nested(Vec<Decoded>),
    Elements(Vec<u64>),
    NullBytes,
    Absent,
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
        mut f: impl FnMut(usize, usize, usize, FieldKind) -> Result<Option<usize>, DecodeError>,
    ) -> Result<usize, DecodeError> {
        let mut cursor = 0;
        let mut size = 0;
        for (index, field) in self.fields.iter().enumerate() {
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
            let Some(width) = f(index, offset, width, field.kind)? else {
                continue;
            };
            cursor = offset
                .checked_add(width)
                .ok_or(DecodeError::LayoutOverflow)?;
            size = size.max(cursor);
        }
        Ok(size.div_ceil(8))
    }

    pub fn byte_len(&self) -> Result<usize, DecodeError> {
        self.visit(|_, _, width, _| Ok(Some(width)))
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

    fn present(&self, index: usize, earlier: &[Decoded]) -> bool {
        self.conditions
            .iter()
            .filter(|(field, _)| *field == index)
            .all(|(_, condition)| condition(earlier))
    }

    fn present_width(
        &self,
        index: usize,
        width: usize,
        kind: FieldKind,
        values: &[Decoded],
    ) -> Result<Option<usize>, DecodeError> {
        if !self.present(index, &values[..index]) {
            return Ok(None);
        }
        if let FieldKind::Nested(layout) = kind {
            let Decoded::Nested(fields) = &values[index] else {
                return Err(DecodeError::InvalidValue);
            };
            return layout
                .encoded_len(fields)?
                .checked_mul(8)
                .map(Some)
                .ok_or(DecodeError::LayoutOverflow);
        }
        Ok(Some(width))
    }

    pub fn encoded_len(&self, values: &[Decoded]) -> Result<usize, DecodeError> {
        if values.len() != self.fields.len() {
            return Err(DecodeError::InvalidValue);
        }
        self.visit(|index, _, width, kind| self.present_width(index, width, kind, values))
    }

    pub fn field_offset(
        &self,
        target: usize,
        values: Option<&[Decoded]>,
    ) -> Result<Option<usize>, DecodeError> {
        if values.is_some_and(|values| values.len() != self.fields.len()) {
            return Err(DecodeError::InvalidValue);
        }
        let mut result = None;
        self.visit(|index, offset, width, kind| {
            let width = if let Some(values) = values {
                self.present_width(index, width, kind, values)?
            } else {
                Some(width)
            };
            if index == target && width.is_some() {
                result = Some(offset / 8);
            }
            Ok(width)
        })?;
        Ok(result)
    }

    pub fn encode(&self, values: &[Decoded]) -> Result<Vec<u8>, DecodeError> {
        let mut data = vec![0; self.encoded_len(values)?];
        self.visit(|index, offset, width, kind| {
            let Some(width) = self.present_width(index, width, kind, values)? else {
                return Ok(None);
            };
            let value = &values[index];
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
            Ok(Some(width))
        })?;
        Ok(data)
    }

    pub fn decode(&self, data: &[u8]) -> Result<Vec<Decoded>, DecodeError> {
        self.decode_with_length(data).map(|(values, _)| values)
    }

    fn decode_with_length(&self, data: &[u8]) -> Result<(Vec<Decoded>, usize), DecodeError> {
        let mut result = Vec::new();
        let length = self.visit(|index, offset, width, kind| {
            if !self.present(index, &result) {
                result.push(Decoded::Absent);
                return Ok(None);
            }
            if !matches!(kind, FieldKind::Nested(_))
                && offset
                    .checked_add(width)
                    .ok_or(DecodeError::LayoutOverflow)?
                    .div_ceil(8)
                    > data.len()
            {
                return Err(DecodeError::InsufficientData);
            }
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
                FieldKind::Nested(layout) => {
                    let remaining = data
                        .get(offset / 8..)
                        .ok_or(DecodeError::InsufficientData)?;
                    let (fields, length) = layout.decode_with_length(remaining)?;
                    result.push(Decoded::Nested(fields));
                    return length
                        .checked_mul(8)
                        .map(Some)
                        .ok_or(DecodeError::LayoutOverflow);
                }
            };
            result.push(value);
            Ok(Some(width))
        })?;
        Ok((result, length))
    }
}

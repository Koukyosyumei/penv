#[derive(Clone, Copy)]
pub struct FieldId(usize);

#[derive(Debug, PartialEq, Eq)]
pub enum FieldError {
    InvalidRange,
    Overlap,
    ValueOutOfRange,
}

type Callback<'a> = Box<dyn FnMut(u64, u64) + 'a>;

struct Field<'a> {
    shift: u32,
    mask: u64,
    on_write: Option<Callback<'a>>,
}

pub struct Register<'a> {
    width: u32,
    defined: u64,
    value: u64,
    fields: Vec<Field<'a>>,
}

impl<'a> Register<'a> {
    pub fn new(width: u32, reset_value: u64) -> Self {
        assert!((1..=64).contains(&width));
        Self {
            width,
            defined: 0,
            value: reset_value,
            fields: Vec::new(),
        }
    }

    pub fn define(&mut self, shift: u32, width: i32) -> Result<FieldId, FieldError> {
        let width = u32::try_from(width).map_err(|_| FieldError::InvalidRange)?;
        if width == 0 || shift.checked_add(width).is_none_or(|end| end > self.width) {
            return Err(FieldError::InvalidRange);
        }
        let mask = u64::MAX >> (64 - width);
        let positioned = mask << shift;
        if self.defined & positioned != 0 {
            return Err(FieldError::Overlap);
        }
        self.defined |= positioned;
        let id = FieldId(self.fields.len());
        self.fields.push(Field {
            shift,
            mask,
            on_write: None,
        });
        Ok(id)
    }

    pub fn on_write(&mut self, id: FieldId, callback: impl FnMut(u64, u64) + 'a) {
        self.fields[id.0].on_write = Some(Box::new(callback));
    }

    pub fn get(&self, id: FieldId) -> u64 {
        let field = &self.fields[id.0];
        (self.value >> field.shift) & field.mask
    }

    pub fn set(&mut self, id: FieldId, value: u64) -> Result<(), FieldError> {
        let field = &self.fields[id.0];
        if value & !field.mask != 0 {
            return Err(FieldError::ValueOutOfRange);
        }
        self.value = (self.value & !(field.mask << field.shift)) | (value << field.shift);
        Ok(())
    }

    pub fn read(&self) -> u64 {
        self.value & self.defined
    }

    pub fn write(&mut self, value: u64) {
        let old = self.value;
        self.value = (old & !self.defined) | (value & self.defined);
        for field in &mut self.fields {
            if let Some(callback) = &mut field.on_write {
                callback(
                    (old >> field.shift) & field.mask,
                    (value >> field.shift) & field.mask,
                );
            }
        }
    }
}

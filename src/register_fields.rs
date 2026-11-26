#[derive(Clone, Copy)]
pub struct FieldId(usize);

#[derive(Debug, PartialEq, Eq)]
pub enum FieldError {
    InvalidRange,
    Overlap,
    ValueOutOfRange,
}

#[derive(Clone, Copy)]
pub enum ReadMode {
    None,
    Value,
    Clear,
    Set,
}

#[derive(Clone, Copy)]
pub enum WriteMode {
    None,
    Value,
    OneClear,
    ZeroClear,
    Clear,
    Set,
    SetBits,
    ZeroSet,
    OneToggle,
    ZeroToggle,
}

type Callback<'a> = Box<dyn FnMut(u64, u64) + 'a>;

#[derive(Default)]
struct Callbacks<'a> {
    read: Option<Callback<'a>>,
    write: Option<Callback<'a>>,
    change: Option<Callback<'a>>,
}

impl Callbacks<'_> {
    fn notify(&mut self, before: u64, input: u64, after: u64, reading: bool) {
        let access = if reading {
            &mut self.read
        } else {
            &mut self.write
        };
        if let Some(callback) = access {
            callback(before, input);
        }
        if before != after {
            if let Some(callback) = &mut self.change {
                callback(before, after);
            }
        }
    }
}

struct Field<'a> {
    shift: u32,
    mask: u64,
    read: ReadMode,
    write: WriteMode,
    callbacks: Callbacks<'a>,
    provider: Option<Box<dyn FnMut(u64) -> u64 + 'a>>,
}

pub struct Register<'a> {
    width: u32,
    defined: u64,
    value: u64,
    reset_value: u64,
    fields: Vec<Field<'a>>,
    callbacks: Callbacks<'a>,
}

impl<'a> Register<'a> {
    pub fn new(width: u32, reset_value: u64) -> Self {
        assert!((1..=64).contains(&width));
        Self {
            width,
            defined: 0,
            value: reset_value,
            reset_value,
            fields: Vec::new(),
            callbacks: Callbacks::default(),
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
            read: ReadMode::Value,
            write: WriteMode::Value,
            callbacks: Callbacks::default(),
            provider: None,
        });
        Ok(id)
    }

    pub fn on_write(&mut self, id: FieldId, callback: impl FnMut(u64, u64) + 'a) {
        self.fields[id.0].callbacks.write = Some(Box::new(callback));
    }

    pub fn observe_field(&mut self, id: FieldId, callback: impl FnMut(u64, u64) + Clone + 'a) {
        self.fields[id.0].callbacks = Callbacks {
            read: Some(Box::new(callback.clone())),
            write: Some(Box::new(callback.clone())),
            change: Some(Box::new(callback)),
        };
    }

    pub fn observe_register(&mut self, callback: impl FnMut(u64, u64) + Clone + 'a) {
        self.callbacks = Callbacks {
            read: Some(Box::new(callback.clone())),
            write: Some(Box::new(callback.clone())),
            change: Some(Box::new(callback)),
        };
    }

    pub fn provide(&mut self, id: FieldId, provider: impl FnMut(u64) -> u64 + 'a) {
        self.fields[id.0].provider = Some(Box::new(provider));
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

    pub fn modes(&mut self, id: FieldId, read: ReadMode, write: WriteMode) {
        self.fields[id.0].read = read;
        self.fields[id.0].write = write;
    }

    pub fn reset(&mut self) {
        self.value = self.reset_value;
    }

    pub fn read(&mut self) -> u64 {
        for field in &mut self.fields {
            if let Some(provider) = &mut field.provider {
                let value = provider((self.value >> field.shift) & field.mask) & field.mask;
                self.value = (self.value & !(field.mask << field.shift)) | (value << field.shift);
            }
        }
        let old = self.value;
        let mut result = 0;
        for field in &self.fields {
            let positioned = field.mask << field.shift;
            if !matches!(field.read, ReadMode::None) {
                result |= self.value & positioned;
            }
            match field.read {
                ReadMode::Clear => self.value &= !positioned,
                ReadMode::Set => self.value |= positioned,
                _ => {}
            }
        }
        self.notify(old, self.value, true);
        result
    }

    pub fn write(&mut self, value: u64) {
        let before = self.value;
        for field in &mut self.fields {
            let old = (self.value >> field.shift) & field.mask;
            let input = (value >> field.shift) & field.mask;
            let new = match field.write {
                WriteMode::None => continue,
                WriteMode::Value => input,
                WriteMode::OneClear => old & !input,
                WriteMode::ZeroClear => old & input,
                WriteMode::Clear => 0,
                WriteMode::Set => field.mask,
                WriteMode::SetBits => old | input,
                WriteMode::ZeroSet => old | (field.mask & !input),
                WriteMode::OneToggle => old ^ input,
                WriteMode::ZeroToggle => old ^ (field.mask & !input),
            };
            self.value = (self.value & !(field.mask << field.shift)) | (new << field.shift);
        }
        self.notify(before, value, false);
    }

    fn notify(&mut self, before: u64, input: u64, reading: bool) {
        // Access handlers run before any field change handlers.
        for field in &mut self.fields {
            let access = if reading {
                &mut field.callbacks.read
            } else {
                &mut field.callbacks.write
            };
            if let Some(callback) = access {
                callback(
                    (before >> field.shift) & field.mask,
                    (input >> field.shift) & field.mask,
                );
            }
        }
        let after = self.value;
        for field in &mut self.fields {
            let old = (before >> field.shift) & field.mask;
            let new = (after >> field.shift) & field.mask;
            if old != new {
                if let Some(callback) = &mut field.callbacks.change {
                    callback(old, new);
                }
            }
        }
        self.callbacks.notify(before, input, after, reading);
    }
}

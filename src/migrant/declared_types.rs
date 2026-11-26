use std::{
    collections::{HashMap, HashSet, hash_map::Entry},
    io::{self, Read, Write},
};

use super::primitive::{Reader, Writer};

/// Type identities for one stamped record's fields in stream order. Each path
/// starts with the declared type and then lists its generic argument identities.
pub struct DeclaredFieldTypes<'a> {
    pub class: &'a str,
    pub fields: &'a [&'a [&'a str]],
}

pub(super) fn fields_for<'a>(
    schemas: &'a [DeclaredFieldTypes<'a>],
    class: &str,
) -> io::Result<&'a [&'a [&'a str]]> {
    schemas
        .iter()
        .find(|schema| schema.class == class)
        .map(|schema| schema.fields)
        .ok_or_else(|| invalid("missing declared field types"))
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[derive(Default)]
pub(super) struct TypeWriter<'a> {
    names: HashMap<&'a str, u64>,
}

impl<'a> TypeWriter<'a> {
    pub(super) fn write(
        &mut self,
        data: &mut Writer<impl Write>,
        fields: &'a [&'a [&'a str]],
    ) -> io::Result<()> {
        data.unsigned(fields.len() as u64)?;
        for field in fields {
            if field.is_empty() {
                return Err(invalid("missing field type identity"));
            }
            data.unsigned(field.len() as u64)?;
            for &name in *field {
                let next = self.names.len() as u64;
                match self.names.entry(name) {
                    Entry::Occupied(entry) => data.unsigned(*entry.get())?,
                    Entry::Vacant(entry) => {
                        data.unsigned(next)?;
                        data.string(name)?;
                        entry.insert(next);
                    }
                }
            }
        }
        Ok(())
    }
}

#[derive(Default)]
pub(super) struct TypeReader {
    names: Vec<String>,
    seen: HashSet<String>,
}

impl TypeReader {
    pub(super) fn verify(
        &mut self,
        data: &mut Reader<impl Read>,
        fields: &[&[&str]],
    ) -> io::Result<()> {
        if data.unsigned()? != fields.len() as u64 {
            return Err(invalid("declared field type count differs"));
        }
        for field in fields {
            if field.is_empty() || data.unsigned()? != field.len() as u64 {
                return Err(invalid("declared generic argument count differs"));
            }
            for &expected in *field {
                let id = usize::try_from(data.unsigned()?)
                    .map_err(|_| invalid("invalid type identity index"))?;
                if id == self.names.len() {
                    let name = data.string()?;
                    if name != expected || !self.seen.insert(name.clone()) {
                        return Err(invalid("invalid declared type definition"));
                    }
                    self.names.push(name);
                } else if self.names.get(id).map(String::as_str) != Some(expected) {
                    return Err(invalid("invalid declared type reference"));
                }
            }
        }
        Ok(())
    }
}

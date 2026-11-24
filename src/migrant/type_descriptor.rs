//! Schema comparison for stamped Migrant fields.
use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Assembly {
    pub name: String,
    pub version: [u32; 4],
    pub culture: String,
    pub public_key_token: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypeIdentity {
    pub name: String,
    pub module: String,
    pub module_guid: [u8; 16],
    pub assembly: Assembly,
}

#[derive(Clone, Copy, Default)]
pub enum VersionTolerance {
    #[default]
    Exact,
    AllowAssemblyVersionChange,
}

impl TypeIdentity {
    fn matches(&self, other: &Self, tolerance: VersionTolerance) -> bool {
        match tolerance {
            VersionTolerance::Exact => self == other,
            VersionTolerance::AllowAssemblyVersionChange => {
                self.name == other.name
                    && self.module == other.module
                    && self.assembly.name == other.assembly.name
                    && self.assembly.culture == other.assembly.culture
                    && self.assembly.public_key_token == other.assembly.public_key_token
            }
        }
    }
}

#[derive(Clone)]
pub struct Field {
    pub name: String,
    pub field_type: TypeIdentity,
    pub transient: bool,
}

#[derive(Clone)]
pub struct TypeDescriptor {
    pub identity: TypeIdentity,
    pub base: Option<Box<TypeDescriptor>>,
    pub fields: Vec<Field>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Differences {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<String>,
}

impl Differences {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

impl TypeDescriptor {
    fn field_index(&self) -> BTreeMap<String, &Field> {
        let mut index = BTreeMap::new();
        let mut current = Some(self);
        while let Some(descriptor) = current {
            for field in &descriptor.fields {
                if !field.transient {
                    index.insert(
                        format!("{}:{}", descriptor.identity.name, field.name),
                        field,
                    );
                }
            }
            current = descriptor.base.as_deref();
        }
        index
    }

    pub fn compare_with(&self, previous: &Self, tolerance: VersionTolerance) -> Differences {
        let mut previous = previous.field_index();
        let mut result = Differences::default();
        for (name, field) in self.field_index() {
            match previous.remove(&name) {
                None => result.added.push(name),
                Some(old) if !field.field_type.matches(&old.field_type, tolerance) => {
                    result.changed.push(name);
                }
                Some(_) => {}
            }
        }
        result.removed = previous.into_keys().collect();
        result
    }
}

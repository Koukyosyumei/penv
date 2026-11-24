use std::collections::HashMap;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Type<'a> {
    Convertible,
    Named(&'a str),
    Object,
    Enumerable(&'a Type<'a>),
    ImplementsEnumerable(&'a Type<'a>),
}

pub fn enumerable_element<'a>(ty: Type<'a>) -> Option<Type<'a>> {
    match ty {
        Type::Enumerable(element) | Type::ImplementsEnumerable(element) => Some(*element),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Access {
    Absent,
    Public,
    NonPublic,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Parameter<'a> {
    pub ty: Type<'a>,
    pub out: bool,
    pub optional: bool,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MemberKind<'a> {
    Method {
        parameters: &'a [Parameter<'a>],
        generic: bool,
        extension: bool,
    },
    Property {
        ty: Type<'a>,
        indices: usize,
        get: Access,
        set: Access,
    },
    Field(Type<'a>),
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Member<'a> {
    pub name: &'a str,
    pub kind: MemberKind<'a>,
    pub is_static: bool,
    pub hidden: bool,
}

macro_rules! queries {
    ($($query:ident),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
        pub enum Query { $($query),+ }
        pub const MEMBER_QUERIES: &[Query] = &[$(Query::$query),+];
    };
}
queries!(
    Static,
    CallableMethod,
    CallableProperty,
    CallableField,
    CallableIndexer,
    ExtensionCallable,
    Gettable,
    Settable,
    Extension
);

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
enum Key<'a> {
    Member(Query, Member<'a>, bool),
    Convertible(Type<'a>),
    Visible(Member<'a>),
}

#[derive(Default)]
pub struct TypeExtensionsCache<'a> {
    values: HashMap<Key<'a>, bool>,
    misses: usize,
}

impl<'a> TypeExtensionsCache<'a> {
    pub fn clear(&mut self) {
        self.values.clear();
        self.misses = 0;
    }

    pub fn misses(&self) -> usize {
        self.misses
    }

    pub fn query(&mut self, query: Query, member: Member<'a>, non_public: bool) -> bool {
        self.lookup(Key::Member(query, member, non_public))
    }

    fn parameters(&mut self, parameters: &[Parameter<'a>]) -> bool {
        parameters
            .iter()
            .all(|p| (!p.out && self.lookup(Key::Convertible(p.ty))) || p.optional)
    }

    fn lookup(&mut self, key: Key<'a>) -> bool {
        if let Some(&result) = self.values.get(&key) {
            return result;
        }
        self.misses += 1;
        use MemberKind::{Field, Method, Property};
        use Query::*;
        let result = match key {
            Key::Visible(member) => !member.hidden,
            Key::Convertible(ty) => match enumerable_element(ty) {
                Some(element) => self.lookup(Key::Convertible(element)),
                None => ty == Type::Convertible,
            },
            Key::Member(query, member, non_public) => match (query, member.kind) {
                (Static, _) => member.is_static,
                (CallableMethod, Method { parameters, .. }) => {
                    self.parameters(parameters) && self.lookup(Key::Visible(member))
                }
                (
                    ExtensionCallable,
                    Method {
                        parameters,
                        generic,
                        ..
                    },
                ) => {
                    !generic
                        && self.parameters(parameters.get(1..).unwrap_or_default())
                        && self.lookup(Key::Visible(member))
                }
                (CallableProperty | CallableIndexer, Property { ty, indices, .. }) => {
                    self.lookup(Key::Convertible(ty))
                        && (indices != 0) == (query == CallableIndexer)
                        && self.lookup(Key::Visible(member))
                }
                (CallableField, Field(ty)) => {
                    self.lookup(Key::Convertible(ty)) && self.lookup(Key::Visible(member))
                }
                (Gettable, Property { get, .. }) => accessible(get, non_public),
                (Settable, Property { set, .. }) => accessible(set, non_public),
                (Extension, Method { extension, .. }) => extension,
                _ => false,
            },
        };
        self.values.insert(key, result);
        result
    }
}

fn accessible(access: Access, non_public: bool) -> bool {
    access == Access::Public || (non_public && access == Access::NonPublic)
}

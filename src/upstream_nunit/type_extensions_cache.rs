use crate::type_extensions::{
    enumerable_element, Access, Member, MemberKind, Parameter, Query, Type, TypeExtensionsCache,
    MEMBER_QUERIES,
};

const EQUALS: Member<'static> = Member {
    name: "System.String.Equals",
    kind: MemberKind::Method {
        parameters: &[Parameter {
            ty: Type::Object,
            out: false,
            optional: false,
        }],
        generic: false,
        extension: false,
    },
    is_static: false,
    hidden: false,
};
const LENGTH: Member<'static> = Member {
    name: "System.String.Length",
    kind: MemberKind::Property {
        ty: Type::Convertible,
        indices: 0,
        get: Access::Public,
        set: Access::Absent,
    },
    is_static: false,
    hidden: false,
};
const EMPTY: Member<'static> = Member {
    name: "System.String.Empty",
    kind: MemberKind::Field(Type::ImplementsEnumerable(&Type::Convertible)),
    is_static: true,
    hidden: false,
};

macro_rules! cases {
    ($($name:ident: $misses:literal, $member:ident, $query:ident;)+) => {
        const TESTED: &[Query] = &[$(Query::$query),+];
        $(
            #[test]
            fn $name() {
                let mut cache = TypeExtensionsCache::default();
                cache.clear();
                cache.query(Query::$query, $member, false);
                cache.query(Query::$query, $member, false);
                assert_eq!(cache.misses(), $misses);
            }
        )+
    };
}
cases! {
    is_static: 1, EQUALS, Static;
    method_callable: 2, EQUALS, CallableMethod;
    property_callable: 3, LENGTH, CallableProperty;
    field_callable: 4, EMPTY, CallableField;
    indexer_callable: 2, LENGTH, CallableIndexer;
    extension_callable: 2, EQUALS, ExtensionCallable;
    currently_gettable: 1, LENGTH, Gettable;
    currently_settable: 1, LENGTH, Settable;
    is_extension: 1, EQUALS, Extension;
}

#[test]
fn enumerable_types() {
    let integer = Type::Named("System.Int32");
    let dictionary = Type::Named("Dictionary<object,double>");
    assert_eq!(
        enumerable_element(Type::ImplementsEnumerable(&integer)),
        Some(integer)
    );
    assert_eq!(
        enumerable_element(Type::Enumerable(&dictionary)),
        Some(dictionary)
    );
}

#[test]
fn all_public_queries_tested() {
    assert_eq!(MEMBER_QUERIES.len() + 1, TESTED.len() + 1);
    assert_eq!(MEMBER_QUERIES, TESTED);
}

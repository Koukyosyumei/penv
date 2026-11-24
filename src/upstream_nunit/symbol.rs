use crate::symbol::Symbol;

fn symbols(inputs: [(u64, u64, &str); 5]) -> [Symbol; 5] {
    inputs.map(|(start, end, name)| Symbol::new(start, end, name).unwrap())
}

#[test]
fn should_overlap() {
    let symbol = symbols([
        (0, 10, "一"),
        (5, 15, "二"),
        (5, 30, "三"),
        (15, 30, "四"),
        (15, 15, "五"),
    ]);
    for (left, right) in [
        (0, 1),
        (1, 0),
        (2, 1),
        (1, 2),
        (2, 3),
        (3, 2),
        (3, 4),
        (4, 3),
    ] {
        assert!(symbol[left].overlaps(&symbol[right]));
    }
}

#[test]
fn should_not_overlap() {
    let symbol = symbols([
        (0, 10, "一"),
        (5, 15, "二"),
        (5, 30, "三"),
        (15, 30, "四"),
        (10, 20, "五"),
    ]);
    for (left, right) in [(0, 4), (4, 0), (0, 3), (3, 0), (1, 3), (3, 1)] {
        assert!(!symbol[left].overlaps(&symbol[right]));
    }
}

fn containment_symbols() -> [Symbol; 5] {
    symbols([
        (5, 30, "三"),
        (5, 15, "二"),
        (15, 30, "四"),
        (10, 20, "五"),
        (15, 15, "一"),
    ])
}

#[test]
fn should_contain() {
    let symbol = containment_symbols();
    for (left, right) in [(0, 0), (0, 1), (0, 2), (0, 3), (2, 4)] {
        assert!(symbol[left].contains(&symbol[right]));
    }
}

#[test]
fn should_not_contain() {
    let symbol = containment_symbols();
    for (left, right) in [
        (1, 0),
        (2, 0),
        (3, 0),
        (1, 2),
        (2, 1),
        (1, 3),
        (3, 1),
        (1, 4),
    ] {
        assert!(!symbol[left].contains(&symbol[right]));
    }
}

#[test]
fn should_demangle() {
    let cases = [
        (
            "_ZN2cl2pl4consC1ERKSs",
            "cl::pl::cons::complete object constructor",
        ),
        ("_Z1hic", "h"),
        ("?h@@YAXHD@Z", "?h@@YAXHD@Z"),
        (
            "_ZN4Test6Ne$ted12staticMemberE3abc",
            "Test::Ne$ted::staticMember",
        ),
        (
            "_ZNSt8ios_base4InitC1Ev@plt",
            "std::ios_base::Init::complete object constructor",
        ),
        (
            "_ZN6tflite12_GLOBAL__N_14EvalEP13TfLiteContextP10TfLiteNode",
            "tflite::(anonymous namespace)::Eval",
        ),
    ];
    for (name, expected) in cases {
        assert_eq!(Symbol::new(0, 0, name).unwrap().name, expected);
    }
}

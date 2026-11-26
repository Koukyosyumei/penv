use crate::access_condition::{Formula, StateMask};
use std::collections::HashMap;

fn dnf(input: &str, expected: &str) {
    assert_eq!(
        Formula::parse(input).unwrap().to_string(),
        expected,
        "Input condition: '{input}'"
    );
}

fn evaluate(position: u8) {
    let bits = HashMap::from([("privileged", 0), ("above32", position)]);
    let result = Formula::parse("above32 && !privileged")
        .unwrap()
        .evaluate(|_, name| bits.get(name).copied())
        .unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(
        result[""],
        [StateMask {
            state: 1 << position,
            mask: (1 << position) | 1
        }]
    );
}

fn conflict(position: u8) {
    let bits = HashMap::from([("above32", position)]);
    let error = Formula::parse("above32 && !above32")
        .unwrap()
        .evaluate(|_, name| bits.get(name).copied())
        .unwrap_err();
    assert!(error.to_string().contains("Conditions conflict detected"));
}

macro_rules! cases {
    ($($name:ident: $function:ident($($argument:expr),+);)+) => {$(
        #[test]
        fn $name() {
            $function($($argument),+);
        }
    )+};
}

cases! {
    detect_conflict_at_bit_32: conflict(32);
    detect_conflict_at_bit_63: conflict(63);
    evaluate_state_bit_31: evaluate(31);
    evaluate_state_bit_32: evaluate(32);
    evaluate_state_bit_33: evaluate(33);
    evaluate_state_bit_63: evaluate(63);
    atom: dnf("a","(a)");
    negated_atom: dnf("!a","(!a)");
    redundant_parentheses: dnf("!((((((a))))))","(!a)");
    double_negation: dnf("!(((!(((a))))))","(a)");
    initiator_and_state: dnf("!secure && initiator == cpu1","(!secure && initiator == cpu1)");
    negated_disjunction: dnf("!(privileged || secure)","(!privileged && !secure)");
    negated_conjunction: dnf("!(privileged && secure)","(!privileged) || (!secure)");
    distribute_initiators: dnf("secure && !privileged && (initiator == cpu1 || initiator == cpu2)","(secure && !privileged && initiator == cpu1) || (secure && !privileged && initiator == cpu2)");
    distribute_two_disjunctions: dnf("(secure || busSecure) && (initiator == cpu1 || initiator == cpu2) && privileged","(secure && initiator == cpu1 && privileged) || (secure && initiator == cpu2 && privileged) || (busSecure && initiator == cpu1 && privileged) || (busSecure && initiator == cpu2 && privileged)");
}

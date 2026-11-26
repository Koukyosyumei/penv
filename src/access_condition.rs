//! Access-condition expressions and their transaction-state masks.
use std::{collections::BTreeMap, fmt};

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Syntax,
    TooComplex,
    MultipleInitiators,
    NegatedInitiator,
    UnknownStateBit(String),
    Conflict(String),
}

impl fmt::Display for Error {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax => output.write_str("Invalid access condition"),
            Self::TooComplex => output.write_str("Access condition is too complex"),
            Self::MultipleInitiators => output.write_str("Multiple initiators in one condition"),
            Self::NegatedInitiator => output.write_str("An initiator condition cannot be negated"),
            Self::UnknownStateBit(name) => write!(output, "Unknown state bit: {name}"),
            Self::Conflict(name) => write!(output, "Conditions conflict detected: {name}"),
        }
    }
}

#[derive(Clone)]
enum Atom<'a> {
    State(&'a str, bool),
    Initiator(&'a str),
}

enum Expression<'a> {
    Atom(Atom<'a>),
    Not(Box<Self>),
    And(Box<Self>, Box<Self>),
    Or(Box<Self>, Box<Self>),
}

struct Parser<'a> {
    remaining: &'a str,
}

impl<'a> Parser<'a> {
    fn take(&mut self, token: &str) -> bool {
        if let Some(remaining) = self.remaining.trim_start().strip_prefix(token) {
            self.remaining = remaining;
            true
        } else {
            false
        }
    }

    fn identifier(&mut self) -> Result<&'a str, Error> {
        let input = self.remaining.trim_start();
        let end = input
            .find(|ch: char| !(ch.is_alphanumeric() || ch == '_'))
            .unwrap_or(input.len());
        if end == 0 {
            return Err(Error::Syntax);
        }
        let (name, remaining) = input.split_at(end);
        self.remaining = remaining;
        Ok(name)
    }

    fn or(&mut self, depth: usize) -> Result<Expression<'a>, Error> {
        let mut left = self.and(depth)?;
        while self.take("||") {
            left = Expression::Or(Box::new(left), Box::new(self.and(depth)?));
        }
        Ok(left)
    }

    fn and(&mut self, depth: usize) -> Result<Expression<'a>, Error> {
        let mut left = self.factor(depth)?;
        while self.take("&&") {
            left = Expression::And(Box::new(left), Box::new(self.factor(depth)?));
        }
        Ok(left)
    }

    fn factor(&mut self, depth: usize) -> Result<Expression<'a>, Error> {
        if depth > 64 {
            return Err(Error::TooComplex);
        }
        if self.take("!") {
            return Ok(Expression::Not(Box::new(self.factor(depth + 1)?)));
        }
        if self.take("(") {
            let expression = self.or(depth + 1)?;
            return if self.take(")") {
                Ok(expression)
            } else {
                Err(Error::Syntax)
            };
        }
        let name = self.identifier()?;
        let atom = if name == "initiator" && self.take("==") {
            Atom::Initiator(self.identifier()?)
        } else {
            Atom::State(name, false)
        };
        Ok(Expression::Atom(atom))
    }
}

pub struct Formula<'a>(Vec<Vec<Atom<'a>>>);

impl<'a> Formula<'a> {
    pub fn parse(input: &'a str) -> Result<Self, Error> {
        // Limit recursion and distribution before allocating generated terms.
        if input.len() > 4096 {
            return Err(Error::TooComplex);
        }
        let mut parser = Parser { remaining: input };
        let expression = parser.or(0)?;
        if !parser.remaining.trim().is_empty() {
            return Err(Error::Syntax);
        }
        Ok(Self(dnf(expression, false)?))
    }

    pub fn evaluate(
        &self,
        mut state_bit: impl FnMut(Option<&str>, &str) -> Option<u8>,
    ) -> Result<BTreeMap<&'a str, Vec<StateMask>>, Error> {
        let mut masks = BTreeMap::<_, Vec<_>>::new();
        for term in &self.0 {
            let mut initiators = term.iter().filter_map(|atom| match atom {
                Atom::Initiator(name) => Some(*name),
                _ => None,
            });
            let initiator = initiators.next();
            if initiators.next().is_some() {
                return Err(Error::MultipleInitiators);
            }
            let mut result = StateMask::default();
            for atom in term {
                if let Atom::State(name, negated) = atom {
                    let bit = state_bit(initiator, name)
                        .and_then(|index| 1u64.checked_shl(index.into()))
                        .ok_or_else(|| Error::UnknownStateBit((*name).into()))?;
                    if result.mask & bit != 0 {
                        return Err(Error::Conflict((*name).into()));
                    }
                    result.mask |= bit;
                    if !negated {
                        result.state |= bit;
                    }
                }
            }
            masks
                .entry(initiator.unwrap_or(""))
                .or_default()
                .push(result);
        }
        Ok(masks)
    }
}

impl fmt::Display for Formula<'_> {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, term) in self.0.iter().enumerate() {
            if index != 0 {
                output.write_str(" || ")?;
            }
            output.write_str("(")?;
            for (index, atom) in term.iter().enumerate() {
                if index != 0 {
                    output.write_str(" && ")?;
                }
                match atom {
                    Atom::State(name, negated) => {
                        if *negated {
                            output.write_str("!")?;
                        }
                        output.write_str(name)?;
                    }
                    Atom::Initiator(name) => write!(output, "initiator == {name}")?,
                }
            }
            output.write_str(")")?;
        }
        Ok(())
    }
}

#[derive(Default, Debug, PartialEq, Eq)]
pub struct StateMask {
    pub state: u64,
    pub mask: u64,
}

fn dnf(expression: Expression<'_>, negated: bool) -> Result<Vec<Vec<Atom<'_>>>, Error> {
    let (left, right, conjunction) = match expression {
        Expression::Atom(Atom::State(name, _)) => {
            return Ok(vec![vec![Atom::State(name, negated)]]);
        }
        Expression::Atom(atom) if !negated => return Ok(vec![vec![atom]]),
        Expression::Atom(_) => return Err(Error::NegatedInitiator),
        Expression::Not(inner) => return dnf(*inner, !negated),
        Expression::And(left, right) => (left, right, !negated),
        Expression::Or(left, right) => (left, right, negated),
    };
    let mut left = dnf(*left, negated)?;
    let right = dnf(*right, negated)?;
    let count = if conjunction {
        left.len().saturating_mul(right.len())
    } else {
        left.len() + right.len()
    };
    if count > 4096 {
        return Err(Error::TooComplex);
    }
    if !conjunction {
        left.extend(right);
        return Ok(left);
    }
    let mut terms = Vec::with_capacity(count);
    for lhs in left {
        for rhs in &right {
            terms.push(lhs.iter().chain(rhs).cloned().collect());
        }
    }
    Ok(terms)
}

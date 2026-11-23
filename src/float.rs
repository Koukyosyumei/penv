// Adapted from penv/rv64 d81737d607dd11ddc593ac22248588c0dc114d12.
// IEEE-754 arithmetic using integer significands so guest rounding and flags
// are independent of the host floating-point environment.
const NX: u8 = 1;
const UF: u8 = 2;
const OF: u8 = 4;
const DZ: u8 = 8;
const NV: u8 = 16;

#[derive(Clone, Copy)]
pub enum Rounding {
    NearestEven,
    TowardZero,
    Down,
    Up,
    NearestAway,
}

impl Rounding {
    pub fn decode(mode: u32, fcsr: u8) -> Option<Self> {
        Some(
            match if mode == 7 {
                u32::from(fcsr >> 5)
            } else {
                mode
            } {
                0 => Self::NearestEven,
                1 => Self::TowardZero,
                2 => Self::Down,
                3 => Self::Up,
                4 => Self::NearestAway,
                _ => return None,
            },
        )
    }

    fn negative_zero(self) -> bool {
        matches!(self, Self::Down)
    }

    fn increment(self, negative: bool, whole: u128, fraction: Fraction) -> bool {
        match self {
            Self::NearestEven => {
                fraction == Fraction::Greater || (fraction == Fraction::Half && whole & 1 != 0)
            }
            Self::NearestAway => matches!(fraction, Fraction::Half | Fraction::Greater),
            Self::TowardZero => false,
            Self::Up => !negative && fraction != Fraction::Zero,
            Self::Down => negative && fraction != Fraction::Zero,
        }
    }

    fn overflow_to_infinity(self, negative: bool) -> bool {
        match self {
            Self::NearestEven | Self::NearestAway => true,
            Self::TowardZero => false,
            Self::Up => !negative,
            Self::Down => negative,
        }
    }
}

#[derive(Clone, Copy)]
pub enum Format {
    Single,
    Double,
}

#[derive(Clone, Copy)]
enum Number {
    Zero,
    Finite { significand: u128, exponent: i32 },
    Infinity,
    Nan { signaling: bool },
}

#[derive(Clone, Copy)]
pub struct Value {
    negative: bool,
    number: Number,
}

#[derive(Clone, Copy)]
pub struct ResultBits {
    pub bits: u64,
    pub flags: u8,
}

impl ResultBits {
    fn new(bits: u64) -> Self {
        Self { bits, flags: 0 }
    }

    fn flagged(bits: u64, flags: u8) -> Self {
        Self { bits, flags }
    }
}

impl Format {
    pub fn width(self) -> usize {
        match self {
            Self::Single => 4,
            Self::Double => 8,
        }
    }

    fn fractional_bits(self) -> u32 {
        match self {
            Self::Single => 23,
            Self::Double => 52,
        }
    }

    fn bias(self) -> i32 {
        match self {
            Self::Single => 127,
            Self::Double => 1023,
        }
    }

    fn sign_mask(self) -> u64 {
        1 << (self.width() * 8 - 1)
    }

    fn fraction_mask(self) -> u64 {
        (1 << self.fractional_bits()) - 1
    }

    pub fn boxed(self, raw: u64) -> u64 {
        match self {
            Self::Single => raw | 0xffff_ffff_0000_0000,
            Self::Double => raw,
        }
    }

    fn zero(self, negative: bool) -> u64 {
        if negative { self.sign_mask() } else { 0 }
    }

    fn infinity(self, negative: bool) -> u64 {
        self.zero(negative) | (((self.bias() * 2 + 1) as u64) << self.fractional_bits())
    }

    fn nan(self) -> u64 {
        self.infinity(false) | (1 << (self.fractional_bits() - 1))
    }

    fn invalid(self) -> ResultBits {
        ResultBits::flagged(self.nan(), NV)
    }

    pub fn unbox(self, bits: u64) -> u64 {
        match self {
            Self::Single if bits >> 32 != u64::from(u32::MAX) => self.nan(),
            Self::Single => u64::from(bits as u32),
            Self::Double => bits,
        }
    }

    pub fn decode(self, bits: u64) -> Value {
        let bits = self.unbox(bits);
        let fraction = bits & self.fraction_mask();
        let exponent = ((bits & !self.sign_mask()) >> self.fractional_bits()) as i32;
        let number = match (exponent, fraction) {
            (0, 0) => Number::Zero,
            (0, bits) => Number::Finite {
                significand: u128::from(bits),
                exponent: 1 - self.bias() - self.fractional_bits() as i32,
            },
            (max, 0) if max == self.bias() * 2 + 1 => Number::Infinity,
            (max, bits) if max == self.bias() * 2 + 1 => Number::Nan {
                signaling: bits & (1 << (self.fractional_bits() - 1)) == 0,
            },
            (exp, bits) => Number::Finite {
                significand: u128::from(bits | (1 << self.fractional_bits())),
                exponent: exp - self.bias() - self.fractional_bits() as i32,
            },
        };
        Value {
            negative: bits & self.sign_mask() != 0,
            number,
        }
    }

    fn round(self, negative: bool, significand: u128, exponent: i32, mode: Rounding) -> ResultBits {
        if significand == 0 {
            return ResultBits::new(self.zero(negative));
        }
        let fraction_bits = self.fractional_bits() as i32;
        let min_exponent = 1 - self.bias();
        let top = bit_len(significand) - 1 + exponent;
        let mut rounded_exponent = top.max(min_exponent);
        let shift = rounded_exponent - fraction_bits - exponent;
        let (mut value, fraction) = split(significand, shift);
        if mode.increment(negative, value, fraction) {
            value += 1;
        }
        let hidden = 1 << fraction_bits;
        if value >= hidden << 1 {
            rounded_exponent += 1;
            value >>= 1;
        }
        if rounded_exponent > self.bias() {
            let infinity = self.infinity(negative);
            let result = if mode.overflow_to_infinity(negative) {
                infinity
            } else {
                infinity - 1
            };
            return ResultBits::flagged(result, OF | NX);
        }
        let normal = value >= hidden;
        let encoded_exponent = if normal {
            (rounded_exponent + self.bias()) as u64
        } else {
            0
        };
        let flags = if fraction == Fraction::Zero {
            0
        } else if top < min_exponent {
            // Detect tininess after rounding to the destination precision
            // with an unbounded exponent, before rounding to a subnormal grid.
            let (whole, remainder) = split(significand, top - fraction_bits - exponent);
            let carries =
                whole + u128::from(mode.increment(negative, whole, remainder)) == hidden << 1;
            if top + i32::from(carries) < min_exponent {
                NX | UF
            } else {
                NX
            }
        } else {
            NX
        };
        ResultBits::flagged(
            self.zero(negative)
                | (encoded_exponent << fraction_bits)
                | (value as u64 & self.fraction_mask()),
            flags,
        )
    }

    pub fn convert(self, value: Value, rounding: Rounding) -> ResultBits {
        match value.number {
            Number::Zero => ResultBits::new(self.zero(value.negative)),
            Number::Infinity => ResultBits::new(self.infinity(value.negative)),
            Number::Nan { signaling } => ResultBits::flagged(self.nan(), u8::from(signaling) * NV),
            Number::Finite {
                significand,
                exponent,
            } => self.round(value.negative, significand, exponent, rounding),
        }
    }

    pub fn add(self, lhs: Value, rhs: Value, rounding: Rounding) -> ResultBits {
        match (lhs.number, rhs.number) {
            (Number::Nan { .. }, _) | (_, Number::Nan { .. }) => {
                ResultBits::flagged(self.nan(), nan_flags(&[lhs, rhs]))
            }
            (Number::Infinity, Number::Infinity) if lhs.negative != rhs.negative => self.invalid(),
            (Number::Infinity, _) => ResultBits::new(self.infinity(lhs.negative)),
            (_, Number::Infinity) => ResultBits::new(self.infinity(rhs.negative)),
            _ => {
                let (negative, significand, exponent) = add_finite(lhs, rhs, rounding);
                self.round(negative, significand, exponent, rounding)
            }
        }
    }

    pub fn multiply(self, lhs: Value, rhs: Value, rounding: Rounding) -> ResultBits {
        match product(lhs, rhs) {
            Some(result) => self.convert(result, rounding),
            None if lhs.is_nan() || rhs.is_nan() => {
                ResultBits::flagged(self.nan(), nan_flags(&[lhs, rhs]))
            }
            None => self.invalid(),
        }
    }

    pub fn fused(self, lhs: Value, rhs: Value, addend: Value, rounding: Rounding) -> ResultBits {
        match product(lhs, rhs) {
            Some(product) => self.add(product, addend, rounding),
            None if lhs.is_nan() || rhs.is_nan() => {
                ResultBits::flagged(self.nan(), nan_flags(&[lhs, rhs, addend]))
            }
            None => self.invalid(),
        }
    }

    pub fn divide(self, lhs: Value, rhs: Value, rounding: Rounding) -> ResultBits {
        let negative = lhs.negative ^ rhs.negative;
        match (lhs.number, rhs.number) {
            (Number::Nan { .. }, _) | (_, Number::Nan { .. }) => {
                ResultBits::flagged(self.nan(), nan_flags(&[lhs, rhs]))
            }
            (Number::Zero, Number::Zero) | (Number::Infinity, Number::Infinity) => self.invalid(),
            (Number::Infinity, _) => ResultBits::new(self.infinity(negative)),
            (Number::Zero, _) | (_, Number::Infinity) => ResultBits::new(self.zero(negative)),
            (_, Number::Zero) => ResultBits::flagged(self.infinity(negative), DZ),
            (
                Number::Finite {
                    significand: numerator,
                    exponent: lhs_exp,
                },
                Number::Finite {
                    significand: denominator,
                    exponent: rhs_exp,
                },
            ) => {
                // Generate more quotient bits than even binary64 can retain.
                let shift = 72 + bit_len(denominator) - bit_len(numerator);
                let scaled = numerator << shift;
                let quotient = scaled / denominator;
                let remainder = !scaled.is_multiple_of(denominator);
                self.round(
                    negative,
                    quotient | u128::from(remainder),
                    lhs_exp - rhs_exp - shift,
                    rounding,
                )
            }
        }
    }

    pub fn sqrt(self, value: Value, rounding: Rounding) -> ResultBits {
        match value.number {
            Number::Nan { .. } | Number::Zero => self.convert(value, rounding),
            _ if value.negative => self.invalid(),
            Number::Infinity => self.convert(value, rounding),
            Number::Finite {
                mut significand,
                mut exponent,
            } => {
                if exponent & 1 != 0 {
                    significand <<= 1;
                    exponent -= 1;
                }
                let scale = (126 - bit_len(significand)) & !1;
                let radicand = significand << scale;
                let root = radicand.isqrt();
                let remainder = root * root != radicand;
                self.round(
                    false,
                    root | u128::from(remainder),
                    (exponent - scale) / 2,
                    rounding,
                )
            }
        }
    }

    pub fn sign_inject(self, lhs: u64, rhs: u64, operation: u32) -> Option<ResultBits> {
        let lhs = self.unbox(lhs);
        let rhs = self.unbox(rhs);
        let sign = match operation {
            0 => rhs,
            1 => !rhs,
            2 => lhs ^ rhs,
            _ => return None,
        };
        Some(ResultBits::new(
            (lhs & !self.sign_mask()) | (sign & self.sign_mask()),
        ))
    }

    pub fn min_max(self, lhs: u64, rhs: u64, maximum: bool) -> ResultBits {
        let lhs_bits = self.unbox(lhs);
        let rhs_bits = self.unbox(rhs);
        let lhs = self.decode(lhs);
        let rhs = self.decode(rhs);
        let bits = match (lhs.is_nan(), rhs.is_nan()) {
            (true, true) => self.nan(),
            (true, false) => rhs_bits,
            (false, true) => lhs_bits,
            (false, false) => {
                let lhs_key = self.order_key(lhs_bits);
                let rhs_key = self.order_key(rhs_bits);
                if (lhs_key >= rhs_key) == maximum {
                    lhs_bits
                } else {
                    rhs_bits
                }
            }
        };
        ResultBits::flagged(bits, nan_flags(&[lhs, rhs]))
    }

    fn order_key(self, bits: u64) -> i64 {
        let magnitude = (bits & !self.sign_mask()) as i64;
        if bits & self.sign_mask() != 0 {
            -magnitude - 1
        } else {
            magnitude
        }
    }

    pub fn compare(self, lhs: u64, rhs: u64, operation: u32) -> Option<ResultBits> {
        if operation > 2 {
            return None;
        }
        let left = self.decode(lhs);
        let right = self.decode(rhs);
        if left.is_nan() || right.is_nan() {
            return Some(ResultBits::flagged(
                0,
                if operation == 2 {
                    nan_flags(&[left, right])
                } else {
                    NV
                },
            ));
        }
        let both_zero = matches!((left.number, right.number), (Number::Zero, Number::Zero));
        let lhs = self.order_key(self.unbox(lhs));
        let rhs = self.order_key(self.unbox(rhs));
        let condition = match operation {
            0 => lhs <= rhs || both_zero,
            1 => lhs < rhs && !both_zero,
            _ => lhs == rhs || both_zero,
        };
        Some(ResultBits::new(u64::from(condition)))
    }

    pub fn class(self, bits: u64) -> u64 {
        let value = self.decode(bits);
        let subnormal = self.unbox(bits) & !(self.sign_mask() | self.fraction_mask()) == 0;
        1 << match value.number {
            Number::Infinity if value.negative => 0,
            Number::Finite { .. } if value.negative && !subnormal => 1,
            Number::Finite { .. } if value.negative => 2,
            Number::Zero if value.negative => 3,
            Number::Zero => 4,
            Number::Finite { .. } if subnormal => 5,
            Number::Finite { .. } => 6,
            Number::Infinity => 7,
            Number::Nan { signaling: true } => 8,
            Number::Nan { signaling: false } => 9,
        }
    }

    pub fn convert_integer(self, value: u64, kind: Integer, rounding: Rounding) -> ResultBits {
        let value = match kind {
            Integer::Word => i128::from(value as i32),
            Integer::UnsignedWord => i128::from(value as u32),
        };
        self.round(value < 0, value.unsigned_abs(), 0, rounding)
    }
}

impl Value {
    pub fn negate(mut self) -> Self {
        self.negative = !self.negative;
        self
    }

    fn is_nan(self) -> bool {
        matches!(self.number, Number::Nan { .. })
    }

    pub fn to_integer(self, kind: Integer, rounding: Rounding) -> ResultBits {
        let (negative_limit, positive_limit) = kind.limits();
        let (magnitude, fraction) = match self.number {
            Number::Zero => (0, Fraction::Zero),
            Number::Nan { .. } => return kind.result(positive_limit, false, NV),
            Number::Infinity => return kind.saturate(self.negative),
            Number::Finite {
                significand,
                exponent,
            } => {
                if bit_len(significand) + exponent > 65 {
                    return kind.saturate(self.negative);
                }
                split(significand, -exponent)
            }
        };
        let magnitude =
            magnitude + u128::from(rounding.increment(self.negative, magnitude, fraction));
        let limit = if self.negative {
            negative_limit
        } else {
            positive_limit
        };
        if magnitude > limit {
            return kind.saturate(self.negative);
        }
        kind.result(
            magnitude,
            self.negative,
            u8::from(fraction != Fraction::Zero) * NX,
        )
    }
}

#[derive(Clone, Copy)]
pub enum Integer {
    Word,
    UnsignedWord,
}

impl Integer {
    pub fn decode(encoding: usize) -> Option<Self> {
        Some(match encoding {
            0 => Self::Word,
            1 => Self::UnsignedWord,
            _ => return None,
        })
    }

    fn limits(self) -> (u128, u128) {
        match self {
            Self::Word => (1 << 31, i32::MAX as u128),
            Self::UnsignedWord => (0, u128::from(u32::MAX)),
        }
    }

    fn saturate(self, negative: bool) -> ResultBits {
        let (lower, upper) = self.limits();
        self.result(if negative { lower } else { upper }, negative, NV)
    }

    fn result(self, magnitude: u128, negative: bool, flags: u8) -> ResultBits {
        let value = magnitude as u64;
        let value = if negative {
            value.wrapping_neg()
        } else {
            value
        };
        let bits = u64::from(value as u32);
        ResultBits::flagged(bits, flags)
    }
}

fn product(lhs: Value, rhs: Value) -> Option<Value> {
    let negative = lhs.negative ^ rhs.negative;
    let number = match (lhs.number, rhs.number) {
        (Number::Nan { .. }, _)
        | (_, Number::Nan { .. })
        | (Number::Zero, Number::Infinity)
        | (Number::Infinity, Number::Zero) => return None,
        (Number::Infinity, _) | (_, Number::Infinity) => Number::Infinity,
        (Number::Zero, _) | (_, Number::Zero) => Number::Zero,
        (
            Number::Finite {
                significand: lhs,
                exponent: a,
            },
            Number::Finite {
                significand: rhs,
                exponent: b,
            },
        ) => Number::Finite {
            significand: lhs * rhs,
            exponent: a + b,
        },
    };
    Some(Value { negative, number })
}

fn add_finite(lhs: Value, rhs: Value, rounding: Rounding) -> (bool, u128, i32) {
    let lhs_parts = match lhs.number {
        Number::Finite {
            significand,
            exponent,
        } => Some((significand, exponent)),
        _ => None,
    };
    let rhs_parts = match rhs.number {
        Number::Finite {
            significand,
            exponent,
        } => Some((significand, exponent)),
        _ => None,
    };
    let ((a, x), (b, y)) = match (lhs_parts, rhs_parts) {
        (Some(a), Some(b)) => (a, b),
        (Some((n, e)), None) => return (lhs.negative, n, e),
        (None, Some((n, e))) => return (rhs.negative, n, e),
        (None, None) => {
            let negative = if lhs.negative == rhs.negative {
                lhs.negative
            } else {
                rounding.negative_zero()
            };
            return (negative, 0, 0);
        }
    };

    // Keep enough high bits for exact cancellation of nearby products and jam
    // any discarded low bits into a sticky bit before the final rounding.
    let exponent = (x + bit_len(a)).max(y + bit_len(b)) - 125;
    let a = align(a, exponent - x);
    let b = align(b, exponent - y);
    if lhs.negative == rhs.negative {
        (lhs.negative, a + b, exponent)
    } else if a == b {
        (rounding.negative_zero(), 0, 0)
    } else if a > b {
        (lhs.negative, a - b, exponent)
    } else {
        (rhs.negative, b - a, exponent)
    }
}

fn bit_len(value: u128) -> i32 {
    (u128::BITS - value.leading_zeros()) as i32
}

fn align(value: u128, shift: i32) -> u128 {
    let (whole, fraction) = split(value, shift);
    whole | u128::from(fraction != Fraction::Zero)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fraction {
    Zero,
    Less,
    Half,
    Greater,
}

fn split(value: u128, shift: i32) -> (u128, Fraction) {
    if shift <= 0 {
        return (value << -shift, Fraction::Zero);
    }
    if shift > 128 {
        return (0, Fraction::Less);
    }
    let whole = value.checked_shr(shift as u32).unwrap_or(0);
    let remainder = value & (u128::MAX >> (128 - shift));
    let half = 1 << (shift - 1);
    let fraction = if remainder == 0 {
        Fraction::Zero
    } else {
        match remainder.cmp(&half) {
            std::cmp::Ordering::Less => Fraction::Less,
            std::cmp::Ordering::Equal => Fraction::Half,
            std::cmp::Ordering::Greater => Fraction::Greater,
        }
    };
    (whole, fraction)
}

fn nan_flags(values: &[Value]) -> u8 {
    u8::from(
        values
            .iter()
            .any(|value| matches!(value.number, Number::Nan { signaling: true })),
    ) * NV
}

use std::cmp::Ordering;
use std::fmt::{Display, Formatter};
use std::hash::{Hash, Hasher};
use std::num::{TryFromIntError};
use std::ops::{Add, Mul, Div, Rem, Sub, Shr, Shl, BitAnd, BitOr, BitXor, Not};
use anyhow::anyhow;
use ruint::{aliases::U256, uint};
use ruint::aliases::U128;
use num_traits::{Zero, One};
use crate::graph::{Operation, TresOperation, UnoOperation};

pub const M: U256 =
    uint!(21888242871839275222246405745257275088548364400416034343698204186575808495617_U256);

// pub const INV: u64 = 14042775128853446655;

// pub const R: U256 = uint!(0x0e0a77c19a07df2f666ea36f7879462e36fc76959f60cd29ac96341c4ffffffb_U256);

#[derive(Copy, Clone, PartialEq, Debug)]
pub struct U64(u64);

impl U64 {
    pub fn as_le_slice(&self) -> Vec<u8> {
        self.0.to_le_bytes().to_vec()
    }
    pub fn new(value: u64) -> Self {
        U64(value)
    }
}

impl TryInto<isize> for U64 {
    type Error = TryFromIntError;

    fn try_into(self) -> Result<isize, Self::Error> {
        self.0.try_into()
    }
}

impl Not for U64 {
    type Output = U64;

    fn not(self) -> Self::Output {
        U64(!self.0)
    }
}

impl Shl<usize> for U64 {
    type Output = U64;

    fn shl(self, rhs: usize) -> Self::Output {
        U64(self.0 << rhs)
    }
}

impl PartialOrd for U64 {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        PartialOrd::<u64>::partial_cmp(&self.0, &other.0)
    }
}

impl BitAnd for U64 {
    type Output = U64;

    fn bitand(self, rhs: Self) -> Self::Output {
        U64(self.0 & rhs.0)
    }
}

impl BitOr for U64 {
    type Output = U64;

    fn bitor(self, rhs: Self) -> Self::Output {
        U64(self.0 | rhs.0)
    }
}

impl BitXor for U64 {
    type Output = U64;

    fn bitxor(self, rhs: Self) -> Self::Output {
        U64(self.0 ^ rhs.0)
    }
}

impl Div for U64 {
    type Output = U64;

    fn div(self, rhs: Self) -> Self::Output {
        U64(self.0 / rhs.0)
    }
}

impl Display for U64 {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Zero for U64 {
    fn zero() -> Self {
        U64(0)
    }

    fn is_zero(&self) -> bool {
        Zero::is_zero(&self.0)
    }
}

impl Mul for U64 {
    type Output = U64;

    fn mul(self, rhs: Self) -> Self::Output {
        U64(self.0 * rhs.0)
    }
}

impl One for U64 {
    fn one() -> Self {
        U64(1)
    }
}

impl Add<Self> for U64 {
    type Output = U64;

    fn add(self, rhs: Self) -> Self::Output {
        U64(self.0 + rhs.0)
    }
}

impl Sub for U64 {
    type Output = U64;

    fn sub(self, rhs: Self) -> Self::Output {
        U64(self.0 - rhs.0)
    }
}

impl Rem for U64 {
    type Output = U64;

    #[inline]
    fn rem(self, rhs: Self) -> Self::Output {
        U64(self.0 % rhs.0)
    }
}

impl Shr<usize> for U64 {
    type Output = U64;

    #[inline]
    fn shr(self, rhs: usize) -> Self::Output {
        U64(self.0 >> rhs)
    }
}

impl Hash for U64 {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl Eq for U64 {
}

impl TryFrom<usize> for U64 {
    type Error = TryFromIntError;

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        Ok(U64(value.try_into()?))
    }
}

impl FieldOps for U64 {
    const BITS: usize = 64;
    const BYTES: usize = 8;
    const MAX: U64 = U64(u64::MAX);

    fn from_str(v: &str) -> Result<Self, Box<dyn std::error::Error>> {
        Ok(U64(v.parse::<u64>()
            .map_err(|e| -> Box<dyn std::error::Error> {Box::new(e)})?))
    }

    fn from_le_bytes(v: &[u8]) -> Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        // If length is less than or equal to 8, proceed normally
        if v.len() <= 8 {
            let mut bytes = [0u8; 8];
            // Copy available bytes
            bytes[..v.len()].copy_from_slice(v);
            return Ok(U64(u64::from_le_bytes(bytes)));
        }

        // Input is longer than 8 bytes, check if extra bytes are all zeros
        for item in v.iter().skip(8) {
            if *item != 0 {
                return Err("Input value too large for U64".into());
            }
        }

        // All extra bytes are zero, use the first 8 bytes
        let bytes: [u8; 8] = v[0..8].try_into()?;
        Ok(U64(u64::from_le_bytes(bytes)))
    }

    fn to_le_bytes(&self) -> Vec<u8> {
        self.0.to_le_bytes().to_vec()
    }

    #[inline]
    fn add_mod(self, rhs: Self, m: Self) -> Self {
        let sum = self.0.wrapping_add(rhs.0);
        U64(if sum >= m.0 || sum < self.0 {
            sum.wrapping_sub(m.0)
        } else {
            sum
        })
    }

    #[inline]
    fn mul_mod(self, rhs: Self, m: Self) -> Self {
        let l = U128::from(self.0);
        let r = U128::from(rhs.0);
        U64(l.mul_mod(r, U128::from(m.0)).try_into().unwrap())
        // let a = self.0 as u128;
        // let b = rhs.0 as u128;
        // let m = m.0 as u128;
        // let result = (a * b) % m;
        // U64(result as u64)
    }

    #[inline]
    fn inv_mod(self, m: Self) -> Self {
        U64(ruint::aliases::U64::from(self.0)
            .inv_mod(ruint::aliases::U64::from(m.0))
            .unwrap()
            .try_into()
            .unwrap())
    }

    #[inline]
    fn pow_mod(self, exp: Self, m: Self) -> Self {
        let b = ruint::aliases::U64::from(self.0);
        let e = ruint::aliases::U64::from(exp.0);
        let m = ruint::aliases::U64::from(m.0);
        let r = b.pow_mod(e, m);
        U64(r.try_into().unwrap())
    }

    fn bit_len(self) -> usize {
        (64 - self.0.leading_zeros()) as usize
    }

    fn from_bool(value: bool) -> Self {
        if value {
            U64(1)
        } else {
            U64(0)
        }
    }

    // fn from_usize(value: usize) -> Self {
    //     U64(u64::try_from(value).unwrap())
    // }
}

pub type U254 = ruint::Uint<254, 4>;

pub const bn254_prime: U254 = uint!(21888242871839275222246405745257275088548364400416034343698204186575808495617_U254);

impl FieldOps for U254 {

    const BITS: usize = 254;
    const BYTES: usize = 32;
    const MAX: U254 = U254::MAX;

    fn from_str(v: &str) -> Result<Self, Box<dyn std::error::Error>> {
        U254::from_str_radix(v, 10).map_err(|e| e.into())
    }

    fn from_le_bytes(v: &[u8]) -> Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        U254::try_from_le_slice(v).ok_or_else(|| anyhow!("invalid le bytes").into())
    }

    fn to_le_bytes(&self) -> Vec<u8> {
        self.to_le_bytes_vec()
    }

    #[inline]
    fn add_mod(self, rhs: Self, m: Self) -> Self {
        <U254>::add_mod(self, rhs, m)
    }

    #[inline]
    fn mul_mod(self, rhs: Self, m: Self) -> Self {
        <U254>::mul_mod(self, rhs, m)
    }

    #[inline]
    fn inv_mod(self, m: Self) -> Self {
        <U254>::inv_mod(self, m).unwrap()
    }

    #[inline]
    fn pow_mod(self, exp: Self, m: Self) -> Self {
        <U254>::pow_mod(self, exp, m)
    }

    fn bit_len(self) -> usize {
        <U254>::bit_len(&self)
    }

    fn from_bool(value: bool) -> Self {
        if value { Self::one() } else { Self::zero() }
    }

    // fn from_usize(value: usize) -> Self {
    //     U254::from(value)
    // }
}

pub trait FieldOps: Sized + Copy + Zero + One + PartialEq + Sub<Output = Self>
    + Shr<usize, Output = Self> + Shl<usize, Output = Self> + Rem<Output = Self>
    + BitAnd<Output = Self> + BitOr<Output = Self> + BitXor<Output = Self>
    + PartialOrd + Not<Output = Self> + TryInto<isize> + Div<Output = Self>
    + Eq + Hash + TryFrom<usize> + Display + Sync + Send {

    const BITS: usize;
    const BYTES: usize;
    const MAX: Self;

    fn from_str(v: &str) -> Result<Self, Box<dyn std::error::Error>>;
    fn from_le_bytes(v: &[u8]) -> Result<Self, Box<dyn std::error::Error + Sync + Send>>;
    fn to_le_bytes(&self) -> Vec<u8>;

    fn add_mod(self, rhs: Self, m: Self) -> Self;
    fn mul_mod(self, rhs: Self, m: Self) -> Self;
    fn inv_mod(self, m: Self) -> Self;
    fn pow_mod(self, exp: Self, m: Self) -> Self;
    fn bit_len(self) -> usize;
    fn from_bool(value: bool) -> Self;
    // fn from_usize(value: usize) -> Self;
}

pub trait FieldOperations: Sync + Send {
    type Type: FieldOps;

    fn parse_str(&self, v: &str) -> Result<Self::Type, Box<dyn std::error::Error>>;
    fn parse_le_bytes(&self, v: &[u8]) -> Result<Self::Type, Box<dyn std::error::Error + Sync + Send>>;
    fn parse_usize(&self, v: usize) -> Result<Self::Type, Box<dyn std::error::Error>>;

    fn op_uno(&self, op: UnoOperation, a: Self::Type) -> Self::Type;
    fn op_duo(&self, op: Operation, a: Self::Type, b: Self::Type) -> Self::Type;
    fn op_tres(&self, op: TresOperation, a: Self::Type, b: Self::Type, c: Self::Type) -> Self::Type;

    fn mul(&self, lhs: Self::Type, rhs: Self::Type) -> Self::Type;
    fn div(&self, lhs: Self::Type, rhs: Self::Type) -> Self::Type;
    fn add(&self, lhs: Self::Type, rhs: Self::Type) -> Self::Type;
    fn sub(&self, lhs: Self::Type, rhs: Self::Type) -> Self::Type;
    fn pow(&self, lhs: Self::Type, rhs: Self::Type) -> Self::Type;
    fn modulo(&self, lhs: Self::Type, rhs: Self::Type) -> Self::Type;
    fn eq(&self, lhs: Self::Type, rhs: Self::Type) -> Self::Type;
    fn neq(&self, lhs: Self::Type, rhs: Self::Type) -> Self::Type;
    fn lt(&self, lhs: Self::Type, rhs: Self::Type) -> Self::Type;
    fn gt(&self, lhs: Self::Type, rhs: Self::Type) -> Self::Type;
    fn lte(&self, lhs: Self::Type, rhs: Self::Type) -> Self::Type;
    fn gte(&self, lhs: Self::Type, rhs: Self::Type) -> Self::Type;
    fn land(&self, lhs: Self::Type, rhs: Self::Type) -> Self::Type;
    fn lor(&self, lhs: Self::Type, rhs: Self::Type) -> Self::Type;
    fn lnot(&self, lhs: Self::Type) -> Self::Type;
    fn shl(&self, lhs: Self::Type, rhs: Self::Type) -> Self::Type;
    fn shr(&self, lhs: Self::Type, rhs: Self::Type) -> Self::Type;
    fn bor(&self, lhs: Self::Type, rhs: Self::Type) -> Self::Type;
    fn band(&self, lhs: Self::Type, rhs: Self::Type) -> Self::Type;
    fn bxor(&self, lhs: Self::Type, rhs: Self::Type) -> Self::Type;
    fn bnot(&self, lhs: Self::Type) -> Self::Type;
    fn idiv(&self, lhs: Self::Type, rhs: Self::Type) -> Self::Type;
    fn neg(&self, lhs: Self::Type) -> Self::Type;
}

#[derive(Clone)]
pub struct Field<T> where T: FieldOps {
    pub prime: T,
    halfPrime: T,
    mask: T,
}

impl<T: FieldOps> Field<T> {
    pub fn new(prime: T) -> Self {
        let primeBits = prime.bit_len();
        let mask = if primeBits == T::BITS {
            T::MAX
        } else {
            (T::one() << primeBits) - T::one()
        };
        Field {
            prime,
            halfPrime: prime >> 1,
            mask,
        }
    }

    fn to_isize(&self, value: T) -> Option<isize> {
        if value > self.halfPrime {
            match TryInto::<isize>::try_into(self.neg(value)) {
                Ok(v) => Some(-v),
                Err(_) => None,
            }
        } else {
            value.try_into().ok()
        }
    }

    fn sqrt(&self, a: T) -> T {
        if a == T::zero() {
            return T::zero();
        }
        let one = T::one();
        let zero = T::zero();
        let mut q = self.prime - one;
        let mut s: usize = 0;
        while q.bitand(one) == zero {
            q = q >> 1;
            s += 1;
        }

        let legendre_exp = (self.prime - one) >> 1;
        let legendre = self.pow(a, legendre_exp);
        if legendre != one {
            return zero;
        }

        if s == 1 {
            let exp = (self.add(q, one)) >> 1;
            let mut r = self.pow(a, exp);
            if r > self.halfPrime {
                r = self.sub(zero, r);
            }
            return r;
        }

        let mut z = self.parse_usize(2).unwrap();
        while self.pow(z, legendre_exp) == one {
            z = self.add(z, one);
        }

        let mut m = s;
        let mut c = self.pow(z, q);
        let mut t = self.pow(a, q);
        let mut r = self.pow(a, (self.add(q, one)) >> 1);

        while t != one {
            let mut t2i = t;
            let mut i = 0usize;
            while t2i != one {
                t2i = self.mul(t2i, t2i);
                i += 1;
                if i == m {
                    return zero;
                }
            }
            let mut b = c;
            for _ in 0..(m - i - 1) {
                b = self.mul(b, b);
            }
            c = self.mul(b, b);
            t = self.mul(t, c);
            r = self.mul(r, b);
            m = i;
        }

        if r > self.halfPrime {
            self.sub(zero, r)
        } else {
            r
        }
    }
}

impl<T: FieldOps> FieldOperations for &Field<T> {
    type Type = T;

    fn parse_str(
        &self, v: &str) -> Result<Self::Type, Box<dyn std::error::Error>> {

        let (v, neg) = if let Some('-') = v.chars().nth(0) {
            (&v[1..], true)
        } else {
            (v, false)
        };

        let mut v = T::from_str(v)?;
        if v >= self.prime {
            v = v % self.prime;
        }

        if neg {
            v = self.sub(Self::Type::zero(), v);
        }

        Ok(v)
    }

    fn parse_le_bytes(&self, v: &[u8]) -> Result<Self::Type, Box<dyn std::error::Error + Sync + Send>> {
        let v = T::from_le_bytes(v)?;
        if v > self.prime {
            Ok(v % self.prime)
        } else {
            Ok(v)
        }
    }

    fn parse_usize(&self, v: usize) -> Result<Self::Type, Box<dyn std::error::Error>> {

        let v = T::try_from(v)
            .map_err(|_| -> Box<dyn std::error::Error> {
                anyhow!("unable to convert usize to field value").into()
            })?;
        if v > self.prime {
            Ok(v % self.prime)
        } else {
            Ok(v)
        }
    }

    fn op_uno(&self, op: UnoOperation, a: T) -> T {
        match op {
            UnoOperation::Neg => self.neg(a),
            UnoOperation::Id => a,
            UnoOperation::Lnot => self.lnot(a),
            UnoOperation::Bnot => self.bnot(a),
            UnoOperation::Sqrt => self.sqrt(a),
        }
    }

    fn op_duo(&self, op: Operation, a: T, b: T) -> T {
        match op {
            Operation::Mul => self.mul(a, b),
            Operation::Div => self.div(a, b),
            Operation::Add => self.add(a, b),
            Operation::Sub => self.sub(a, b),
            Operation::Pow => self.pow(a, b),
            Operation::Idiv => self.idiv(a, b),
            Operation::Mod => self.modulo(a, b),
            Operation::Eq => self.eq(a, b),
            Operation::Neq => self.neq(a, b),
            Operation::Lt => self.lt(a, b),
            Operation::Gt => self.gt(a, b),
            Operation::Leq => self.lte(a, b),
            Operation::Geq => self.gte(a, b),
            Operation::Land => self.land(a, b),
            Operation::Lor => self.lor(a, b),
            Operation::Shl => self.shl(a, b),
            Operation::Shr => self.shr(a, b),
            Operation::Bor => self.bor(a, b),
            Operation::Band => self.band(a, b),
            Operation::Bxor => self.bxor(a, b),
        }
    }

    fn op_tres(&self, op: TresOperation, a: T, b: T, c: T) -> T {
        match op {
            TresOperation::TernCond => if a.is_zero() { c } else { b }
        }
    }

    #[inline]
    fn mul(&self, lhs: Self::Type, rhs: Self::Type) -> Self::Type {
        lhs.mul_mod(rhs, self.prime)
    }

    #[inline]
    fn div(&self, lhs: Self::Type, rhs: Self::Type) -> Self::Type {
        if rhs == T::zero() {
            return T::zero();
        }
        let inv = rhs.inv_mod(self.prime);
        lhs.mul_mod(inv, self.prime)
    }

    #[inline]
    fn add(&self, lhs: T, rhs: T) -> T {
        lhs.add_mod(rhs, self.prime)
    }

    #[inline]
    fn sub(&self, lhs: Self::Type, rhs: Self::Type) -> Self::Type {
        lhs.add_mod(self.prime - rhs, self.prime)
    }

    #[inline]
    fn pow(&self, lhs: Self::Type, rhs: Self::Type) -> Self::Type {
        lhs.pow_mod(rhs, self.prime)
    }

    #[inline]
    fn modulo(&self, lhs: Self::Type, rhs: Self::Type) -> Self::Type {
        if rhs.is_zero() {
            // Keep witness evaluation total, matching the zero-divisor convention
            // used by div/idiv.
            Self::Type::zero()
        } else {
            lhs % rhs
        }
    }

    #[inline]
    fn eq(&self, lhs: Self::Type, rhs: Self::Type) -> Self::Type {
        if lhs == rhs {
            Self::Type::one()
        } else {
            Self::Type::zero()
        }
    }

    #[inline]
    fn neq(&self, lhs: Self::Type, rhs: Self::Type) -> Self::Type {
        if lhs != rhs {
            Self::Type::one()
        } else {
            Self::Type::zero()
        }
    }

    #[inline]
    fn lt(&self, lhs: Self::Type, rhs: Self::Type) -> Self::Type {
        let lhs_neg = self.halfPrime < lhs;
        let rhs_neg = self.halfPrime < rhs;

        if lhs_neg == rhs_neg {
            T::from_bool(lhs < rhs)
        } else if lhs_neg {
            T::one()
        } else {
            T::zero()
        }
    }

    #[inline]
    fn gt(&self, lhs: Self::Type, rhs: Self::Type) -> Self::Type {
        let lhs_neg = self.halfPrime < lhs;
        let rhs_neg = self.halfPrime < rhs;

        if lhs_neg == rhs_neg {
            T::from_bool(lhs > rhs)
        } else if lhs_neg {
            T::zero()
        } else {
            T::one()
        }
    }

    #[inline]
    fn lte(&self, lhs: Self::Type, rhs: Self::Type) -> Self::Type {
        let lhs_neg = self.halfPrime < lhs;
        let rhs_neg = self.halfPrime < rhs;

        if lhs_neg == rhs_neg {
            T::from_bool(lhs <= rhs)
        } else if lhs_neg {
            T::one()
        } else {
            T::zero()
        }
    }

    #[inline]
    fn gte(&self, lhs: Self::Type, rhs: Self::Type) -> Self::Type {
        let lhs_neg = self.halfPrime < lhs;
        let rhs_neg = self.halfPrime < rhs;

        if lhs_neg == rhs_neg {
            T::from_bool(lhs >= rhs)
        } else if lhs_neg {
            T::zero()
        } else {
            T::one()
        }
    }

    #[inline]
    fn land(&self, lhs: Self::Type, rhs: Self::Type) -> Self::Type {
        T::from_bool(!lhs.is_zero() && !rhs.is_zero())
    }

    #[inline]
    fn lor(&self, lhs: Self::Type, rhs: Self::Type) -> Self::Type {
        T::from_bool(!lhs.is_zero() || !rhs.is_zero())
    }

    #[inline]
    fn lnot(&self, lhs: Self::Type) -> Self::Type {
        if lhs.is_zero() {
            T::one()
        } else {
            T::zero()
        }
    }

    #[inline]
    fn shl(&self, lhs: Self::Type, rhs: Self::Type) -> Self::Type {
        match self.to_isize(rhs) {
            Some(r) => {
                let mut r = if r >= 0 {
                    lhs << r as usize
                } else {
                    lhs >> (-r as usize)
                };
                if r > self.prime {
                    r = r % self.prime
                }
                r
            },
            None => {
                T::zero()
            }
        }
    }

    #[inline]
    fn shr(&self, lhs: Self::Type, rhs: Self::Type) -> Self::Type {
        match self.to_isize(rhs) {
            Some(r) => {
                let mut r = if r >= 0 {
                    lhs >> r as usize
                } else {
                    lhs << (-r as usize)
                };
                if r > self.prime {
                    r = r % self.prime
                }
                r
            },
            None => {
                T::zero()
            }
        }
    }

    #[inline]
    fn bor(&self, lhs: Self::Type, rhs: Self::Type) -> Self::Type {
        let r = lhs | rhs;
        if r >= self.prime {
            r % self.prime
        } else {
            r
        }
    }

    #[inline]
    fn band(&self, lhs: Self::Type, rhs: Self::Type) -> Self::Type {
        lhs & rhs
    }

    #[inline]
    fn bxor(&self, lhs: Self::Type, rhs: Self::Type) -> Self::Type {
        let r = lhs ^ rhs;
        if r >= self.prime {
            r % self.prime
        } else {
            r
        }
    }
    #[inline]
    fn bnot(&self, lhs: Self::Type) -> Self::Type {
        let lhs = lhs.not();
        let lhs = lhs & self.mask;
        if lhs >= self.prime {
            lhs % self.prime
        } else {
            lhs
        }
    }

    #[inline]
    fn idiv(&self, lhs: Self::Type, rhs: Self::Type) -> Self::Type {
        if rhs.is_zero() {
            T::zero()
        } else {
            lhs / rhs
        }
    }

    #[inline]
    fn neg(&self, lhs: Self::Type) -> Self::Type {
        if lhs.is_zero() {
            T::zero()
        } else {
            self.prime - lhs
        }
    }
}

// fn get_field(prime: U256) -> Box<dyn FieldOperations> {
//     let bn254_prime = uint!(21888242871839275222246405745257275088548364400416034343698204186575808495617_U256);
//     let goldilocks_prime = uint!(18446744069414584321_U256);
//     match prime {
//         bn254_prime => {
//             let prime = U254::from_str_radix(
//                  "21888242871839275222246405745257275088548364400416034343698204186575808495617",
//                  10).unwrap();
//             let halfPrime = prime.shr(1);
//             Field::<U254> {
//                 prime,
//                 halfPrime,
//             }
//         }
//         goldilocks_prime => {
//             let prime = U64(18446744069414584321);
//             let halfPrime = U64(prime.0 >> 1);
//             Field::<U64> {
//                 prime,
//                 halfPrime,
//             }
//         }
//     }
// }

#[cfg(test)]
mod tests {
    use crate::field::{Field, U64, FieldOperations, U254, FieldOps, bn254_prime};
    use num_traits::One;
    use ruint::aliases::U256;

    #[test]
    fn test_gl_add() {
        let a = U64(18446744069414584320);
        let b = U64(18446744069414584320);
        let ff = Field::new(U64(18446744069414584321));
        let result = (&ff).add(a, b);
        assert_eq!(U64(18446744069414584319), result);
    }

    #[test]
    fn test_gl_mul() {
        let a = U64(18446744069414584320);
        let b = U64(18446744069414584320);
        let ff = Field::new(U64(18446744069414584321));
        let result = (&ff).mul(a, b);
        assert_eq!(U64(1), result);
    }

    #[test]
    fn test_gl_div() {
        let a = U64(2);
        let b = U64(3);
        let ff = &Field::<U64>::new(U64(18446744069414584321));
        let result = ff.div(a, b);
        assert_eq!(U64(6148914689804861441), result);
    }

    #[test]
    fn test_gl_shr() {
        let a = U64(256);
        let b = U64(2);
        let want = U64(64);
        let ff = &Field::<U64>::new(U64(18446744069414584321));
        let result = ff.shr(a, b);
        assert_eq!(want, result);
    }

    fn ff_bn256() -> Field<U254> {
        let prime = U254::from_str_radix(
            "21888242871839275222246405745257275088548364400416034343698204186575808495617",
            10)
            .unwrap();
        Field::<U254>::new(prime)
    }

    #[test]
    fn test_bn254_shr() {
        let a = U254::from(256);
        let b = U254::from(2);
        let want = U254::from(64);
        let ff = &ff_bn256();
        let result = ff.shr(a, b);
        assert_eq!(want, result);
    }

    fn u254(s: &str) -> U254 {
        let x = U254::from_str_radix(s, 10).expect(s);
        let prime = U254::from_str_radix(
            "21888242871839275222246405745257275088548364400416034343698204186575808495617",
            10)
            .unwrap();
        if x > prime {
            panic!("{}", s)
        };
        x
        // match U254::from_str_radix(s, 10) {
        //     Ok(r) => r,
        //     Err(_) => {
        //         let x = U256::from_str_radix(s, 10).unwrap();
        //         let y = U256::from_str_radix("21888242871839275222246405745257275088548364400416034343698204186575808495617", 10).unwrap();
        //         U254::from_limbs((x % y).into_limbs())
        //     }
        // }
    }

    #[test]
    fn test_bn254_lt() {
        let ff = &ff_bn256();
        let test_cases: &[(U254, U254, U254)] = &[
            (u254("41456"), u254("7096"), u254("0")),
            (u254("2147483647"), u254("2147483647"), u254("0")),
            (u254("9915499612839321149637521777990102151350674507940716049588462388200839649614"), u254("2"), u254("0")),
            (u254("9915499612839321149637521777990102151350674507940716049588462388200839649614"), u254("19830999225678642299275043555980204302701349015881432099176924776401679299228"), u254("0")),
            (u254("0"), u254("19830999225678642299275043555980204302701349015881432099176924776401679299228"), u254("0")),
            (u254("11972743258999954072608883967267172937197689892475318294109741798374968846004"), u254("11972743258999954072608883967267172937197689892475318294109741798374968846004"), u254("0")),
            (u254("1"), u254("19830999225678642299275043555980204302701349015881432099176924776401679299228"), u254("0")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("6350874878119819312338956282401532410528162663560392320966563075034087161851"), u254("0")),
            (u254("65535"), u254("11972743258999954072608883967267172937197689892475318294109741798374968846004"), u254("0")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("7096"), u254("0")),
            // (u254("65535"), u254("115792089237316195423570985008687907853269984665640564039457584007913129639935"), u254("0")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("944936681149208446651664254269745548490766851729442924617792859073125903783"), u254("0")),
            // (u254("115792089237316195423570985008687907853269984665640564039457584007913129639935"), u254("115792089237316195423570985008687907853269984665640564039457584007913129639935"), u254("0")),
            // (u254("115792089237316195423570985008687907853269984665640564039457584007913129639935"), u254("11972743258999954072608883967267172937197689892475318294109741798374968846004"), u254("0")),
            (u254("16930493065419614647427644856262224012873027146445676318903972992475388670810"), u254("944936681149208446651664254269745548490766851729442924617792859073125903783"), u254("1")),
            // (u254("115792089237316195423570985008687907853269984665640564039457584007913129639935"), u254("65535"), u254("1")),
            // (u254("11972743258999954072608883967267172937197689892475318294109741798374968846004"), u254("115792089237316195423570985008687907853269984665640564039457584007913129639935"), u254("1")),
            (u254("1"), u254("2"), u254("1")),
            (u254("16930493065419614647427644856262224012873027146445676318903972992475388670810"), u254("6350874878119819312338956282401532410528162663560392320966563075034087161851"), u254("1")),
            (u254("41456"), u254("6350874878119819312338956282401532410528162663560392320966563075034087161851"), u254("1")),
            (u254("16930493065419614647427644856262224012873027146445676318903972992475388670810"), u254("7096"), u254("1")),
            (u254("11972743258999954072608883967267172937197689892475318294109741798374968846004"), u254("65535"), u254("1")),
            (u254("0"), u254("2"), u254("1")),
            (u254("41456"), u254("944936681149208446651664254269745548490766851729442924617792859073125903783"), u254("1")),
        ];
        for (a, b, r) in test_cases {
            let r2 = ff.lt(*a, *b);
            assert_eq!(*r, r2, "{} < {} != {}", a, b, r);
        }
    }

    // fn print_u256_as_bits(value: U254) {
    //     // Print the bits starting from the most significant bit (bit 255)
    //     for i in (0..256).rev() {
    //         // Extract bit at position i
    //         let bit:U254 = (value >> i) & U254::from(1);
    //
    //         print!("{}", if bit.is_zero() { "0" } else { "1" });
    //
    //         // Add a space every 8 bits for readability
    //         if i % 8 == 0 && i != 0 {
    //             print!(" ");
    //         }
    //
    //         // Add a separator every 64 bits (at u64 limb boundaries)
    //         if i % 64 == 0 && i != 0 {
    //             print!("| ");
    //         }
    //     }
    //     println!();
    // }

    #[test]
    fn test_ok() {
        let primes = vec![
            "21888242871839275222246405745257275088548364400416034343698204186575808495617",
            "52435875175126190479447740508185965837690552500527637822603658699938581184513",
            "18446744069414584321",
            "21888242871839275222246405745257275088696311157297823662689037894645226208583",
            "28948022309329048855892746252171976963363056481941560715954676764349967630337",
            "28948022309329048855892746252171976963363056481941647379679742748393362948097",
            "115792089210356248762697446949407573530086143415290314195533631308867097853951",
            "8444461749428370424248824938781546531375899335154063827935233455917409239041",
        ];
        for p in primes {
            let i = U256::from_str_radix(p, 10).unwrap();
            println!("{}: {}", i.bit_len(), i)
        }
    }

    #[test]
    fn test_bn254_bnot() {
        let test_cases: &[(U254, U254)] = &[
            (u254("0"), u254("7059779437489773633646340506914701874769131765994106666166191815402473914366")),
            (u254("2147483647"), u254("7059779437489773633646340506914701874769131765994106666166191815400326430719")),
            (u254("21888242871839275222246405745257275088548364400416034343698204186575808495616"), u254("7059779437489773633646340506914701874769131765994106666166191815402473914367")),
            (u254("1431655765"), u254("7059779437489773633646340506914701874769131765994106666166191815401042258601")),
            (u254("21888242871839275222246405745257275088548364400416034343698204186574090508698"), u254("7059779437489773633646340506914701874769131765994106666166191815404191901285")),
            // (u254("115792089237316195423570985008687907853269984665640564039457584007913129639935"), u254("0")),
            (u254("7059779437489773633646340506914701874769131765994106666166191815402473914366"), u254("0")),
            // (u254("38597363079105398474523661669562635951089994888546854679819194669304376546645"), u254("19298681539552699237261830834781317975544997444273427339909597334652188273322")),
            // (u254("69475253542389717254142591005212744711961990799384338423674550404747877783961"), u254("17368813385597429313535647751303186177990497699846084605918637601186969445990")),
            (u254("11972743258999954072608883967267172937197689892475318294109741798374968846004"), u254("16975279050329094783283862284904804026119806273934822715754654203603313563979")),
            (u254("18583076334226168172367231819260574371431472897769128993835383390508861945746"), u254("10364945975102880683525514432911402591886023268641012016029012611469420464237")),
            (u254("2805997381032117399116049231308848744608941055401984107726204241709819608479"), u254("4253782056457656234530291275605853130160190710592122558439987573692654305887")),
        ];
        let ff = &ff_bn256();
        for (x, want) in test_cases {
            let result = ff.bnot(*x);
            assert_eq!(*want, result, "bnot({}) != {}", x, want);
        }
    }

    #[test]
    fn test_bn254_lnot() {
        let test_cases: &[(U254, U254)] = &[
            (u254("1"), u254("0")),
            (u254("0"), u254("1")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("0")),
            // (u254("115792089237316195423570985008687907853269984665640564039457584007913129639935"), u254("0")),
        ];
        let ff = &ff_bn256();
        for (x, want) in test_cases {
            let result = ff.lnot(*x);
            assert_eq!(*want, result, "lnot({}) != {}", x, want);
        }
    }

    #[test]
    fn test_bn254_neg() {
        let test_cases: &[(U254, U254)] = &[
            (u254("41456"), u254("21888242871839275222246405745257275088548364400416034343698204186575808454161")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("10944121435919637611123202872628637544274182200208017171849102093287904247809")),
            (u254("16930493065419614647427644856262224012873027146445676318903972992475388670810"), u254("4957749806419660574818760888995051075675337253970358024794231194100419824807")),
            (u254("21888242871839275222246405745257275088548364400416034343698204186573661011969"), u254("2147483648")),
            (u254("2147483647"), u254("21888242871839275222246405745257275088548364400416034343698204186573661011970")),
        ];
        let ff = &ff_bn256();
        for (x, want) in test_cases {
            let result = ff.neg(*x);
            assert_eq!(*want, result, "neg({}) != {}", x, want);
        }
    }

    #[test]
    fn test_bn254_shl() {
        let test_cases: &[(U254, U254, U254)] = &[
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("0"), u254("10944121435919637611123202872628637544274182200208017171849102093287904247808")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("1"), u254("21888242871839275222246405745257275088548364400416034343698204186575808495616")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("10860587405108423676951016462081417603544482648278432775055937810365549849010"), u254("0")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("12"), u254("15582866685468026238667767924679042131566226449140052623837163044874628366336")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("132"), u254("733698875131086504241680637964758360712637197649879930120766481639896252416")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("13659335715278406968201490888483447219504749355177513827295765003365320703946"), u254("0")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("15288472737928068305360287766555487116560329349262636401065935320480071987933"), u254("0")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("17439619456273153639614935724514253753935535423628492267482988234538132507645"), u254("0")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("18446744073709551184"), u254("0")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("18446744073709551284"), u254("0")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("18446744073709551384"), u254("0")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("18446744073709551484"), u254("0")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("18446744073709551584"), u254("0")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("18446744073709551594"), u254("0")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("18446744073709551604"), u254("0")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("18446744073709551611"), u254("0")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("18446744073709551615"), u254("0")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("1997599621687373223"), u254("0")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("21888242871839275222246405745257275088548364400416034343698204186575808495185"), u254("0")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("21888242871839275222246405745257275088548364400416034343698204186575808495285"), u254("0")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("21888242871839275222246405745257275088548364400416034343698204186575808495385"), u254("1585703")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("21888242871839275222246405745257275088548364400416034343698204186575808495485"), u254("2010117644161974282558189296413313730")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("21888242871839275222246405745257275088548364400416034343698204186575808495585"), u254("2548126838151281143334508611035681689221920026514683191629383269065")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("21888242871839275222246405745257275088548364400416034343698204186575808495595"), u254("2609281882266911890774536817700538049763246107151035588228488467523552")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("21888242871839275222246405745257275088548364400416034343698204186575808495605"), u254("2671904647441317776153125701325350962957564013722660442345972190744117248")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("21888242871839275222246405745257275088548364400416034343698204186575808495612"), u254("342003794872488675347600089769644923258568193756500536620284440415247007744")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("21888242871839275222246405745257275088548364400416034343698204186575808495616"), u254("5472060717959818805561601436314318772137091100104008585924551046643952123904")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("22"), u254("6495193478952948798891169924579835935875496227426190373972760861585839226880")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("232"), u254("0")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("32"), u254("92770739628106842872705277111998648253150376080617345462233624296887943167")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("432"), u254("0")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("5"), u254("2835618237479817285229536898052677856963876409734857380798514961473547010048")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("944936681149208446651664254269745548490766851729442924617792859073125903783"), u254("0")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("944936681149208446651664254269745548490766851729442924633933760137621761447"), u254("0")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("9544780994875477684418232609594850373466272093208420876570527725531033317283"), u254("0")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("9544848422957141122258766509929305647810718586108309338253206183017062311168"), u254("0")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("9915499612839321149637521777990102151350674507940716049588462388200839649614"), u254("0")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("9915567040920984587478055678324557425695121000840604511271140845686868643499"), u254("0")),
            (u254("16930493065419614647427644856262224012873027146445676318903972992475388670810"), u254("0"), u254("16930493065419614647427644856262224012873027146445676318903972992475388670810")),
            (u254("16930493065419614647427644856262224012873027146445676318903972992475388670810"), u254("1"), u254("4912963821510180438962543460352471062428558126481211627943549982972494931636")),
            (u254("16930493065419614647427644856262224012873027146445676318903972992475388670810"), u254("12"), u254("16786165115669586000506057298184729582515873289202483605444952441205623726080")),
            (u254("16930493065419614647427644856262224012873027146445676318903972992475388670810"), u254("132"), u254("4697223465521920117666830470784637317043837089222885686291739583581006069759")),
            (u254("16930493065419614647427644856262224012873027146445676318903972992475388670810"), u254("17517134915284567949426085140067948367061149336705605074337744129697713915893"), u254("0")),
            (u254("16930493065419614647427644856262224012873027146445676318903972992475388670810"), u254("17906618691481904994686607098016695462657290520749280949403472028205799253631"), u254("0")),
            (u254("16930493065419614647427644856262224012873027146445676318903972992475388670810"), u254("21888242871839275222246405745257275088548364400416034343698204186575808495185"), u254("0")),
            (u254("16930493065419614647427644856262224012873027146445676318903972992475388670810"), u254("21888242871839275222246405745257275088548364400416034343698204186575808495485"), u254("3109640461724479741980360722708845210")),
            (u254("16930493065419614647427644856262224012873027146445676318903972992475388670810"), u254("21888242871839275222246405745257275088548364400416034343698204186575808495585"), u254("3941937597799025161058559281812567266838864224600995965046801835409")),
            (u254("16930493065419614647427644856262224012873027146445676318903972992475388670810"), u254("21888242871839275222246405745257275088548364400416034343698204186575808495616"), u254("8465246532709807323713822428131112006436513573222838159451986496237694335405")),
            (u254("16930493065419614647427644856262224012873027146445676318903972992475388670810"), u254("32"), u254("14387363916943729614106330702133940593981074761309425099105946753267631390720")),
            (u254("16930493065419614647427644856262224012873027146445676318903972992475388670810"), u254("432"), u254("0")),
            (u254("16930493065419614647427644856262224012873027146445676318903972992475388670810"), u254("6350874878119819312338956282401532410528162663560392320966563075034087161851"), u254("0")),
            (u254("41456"), u254("0"), u254("41456")),
            (u254("41456"), u254("1"), u254("82912")),
            (u254("41456"), u254("12"), u254("169803776")),
            (u254("41456"), u254("2"), u254("165824")),
            (u254("41456"), u254("21888242871839275222246405745257275088548364400416034343698204186575808495595"), u254("0")),
            (u254("41456"), u254("21888242871839275222246405745257275088548364400416034343698204186575808495605"), u254("10")),
            (u254("41456"), u254("21888242871839275222246405745257275088548364400416034343698204186575808495612"), u254("1295")),
            (u254("41456"), u254("21888242871839275222246405745257275088548364400416034343698204186575808495615"), u254("10364")),
            (u254("41456"), u254("21888242871839275222246405745257275088548364400416034343698204186575808495616"), u254("20728")),
            (u254("41456"), u254("256"), u254("0")),
            (u254("41456"), u254("7096"), u254("0")),
        ];
        let ff = &ff_bn256();
        for (a, b, want) in test_cases {
            let result = ff.shl(*a, *b);
            assert_eq!(*want, result, "{} << {} != {}", a, b, want);
        }
    }

    #[test]
    fn test_bn254_bor() {
        let test_cases: &[(U254, U254, U254)] = &[
            (u254("0"), u254("0"), u254("0")),
            (u254("0"), u254("1431655765"), u254("1431655765")),
            (u254("0"), u254("16704042317112023384305813873602242467992523444124729936442177220570557996694"), u254("16704042317112023384305813873602242467992523444124729936442177220570557996694")),
            (u254("0"), u254("2147483647"), u254("2147483647")),
            (u254("0"), u254("21888242871839275222246405745257275088512165998870179052969728296241183326209"), u254("21888242871839275222246405745257275088512165998870179052969728296241183326209")),
            (u254("0"), u254("21888242871839275222246405745257275088548364400416034343698204186574090508698"), u254("21888242871839275222246405745257275088548364400416034343698204186574090508698")),
            (u254("0"), u254("21888242871839275222246405745257275088548364400416034343698204186575629538646"), u254("21888242871839275222246405745257275088548364400416034343698204186575629538646")),
            (u254("0"), u254("21888242871839275222246405745257275088548364400416034343698204186575701121434"), u254("21888242871839275222246405745257275088548364400416034343698204186575701121434")),
            (u254("0"), u254("21888242871839275222246405745257275088548364400416034343698204186575808495616"), u254("21888242871839275222246405745257275088548364400416034343698204186575808495616")),
            (u254("0"), u254("26285948001224968191279401605282656404690027600887232916913378662810"), u254("26285948001224968191279401605282656404690027600887232916913378662810")),
            (u254("0"), u254("4116010325"), u254("4116010325")),
            (u254("0"), u254("4187593113"), u254("4187593113")),
            (u254("0"), u254("7834155660356680066307500141620057836909020373249803374048008616669345161152"), u254("7834155660356680066307500141620057836909020373249803374048008616669345161152")),
            (u254("1431655765"), u254("0"), u254("1431655765")),
            (u254("1431655765"), u254("21888242871839275222246405745257275088548364400416034343698204186574090508698"), u254("21888242871839275222246405745257275088548364400416034343698204186575504268767")),
            (u254("16704042317112023384305813873602242467992523444124729936442177220570557996694"), u254("2147483647"), u254("16704042317112023384305813873602242467992523444124729936442177220571234828287")),
            (u254("16704042317112023384305813873602242467992523444124729936442177220570557996694"), u254("21888242871839275222246405745257275088548364400416034343698204186574090508698"), u254("2053304182759865277002044103545550374889715499369016654264909112099837960093")),
            (u254("16704042317112023384305813873602242467992523444124729936442177220570557996694"), u254("26285948001224968191279401605282656404690027600887232916913378662810"), u254("16704042342551936977365714893961540918536437843192385768022554216171182874526")),
            (u254("16704042317112023384305813873602242467992523444124729936442177220570557996694"), u254("7834155660356680066307500141620057836909020373249803374048008616669345161152"), u254("2536648999982979338372617759900832617899818347007980716302112949324769877973")),
            (u254("2147483647"), u254("0"), u254("2147483647")),
            (u254("2147483647"), u254("16704042317112023384305813873602242467992523444124729936442177220570557996694"), u254("16704042317112023384305813873602242467992523444124729936442177220571234828287")),
            (u254("2147483647"), u254("21888242871839275222246405745257275088548364400416034343698204186575808495616"), u254("268435454")),
            (u254("2147483647"), u254("26285948001224968191279401605282656404690027600887232916913378662810"), u254("26285948001224968191279401605282656404690027600887232916913754472447")),
            (u254("21888242871839275222246405745257275088512165998870179052969728296241183326209"), u254("0"), u254("21888242871839275222246405745257275088512165998870179052969728296241183326209")),
            (u254("21888242871839275222246405745257275088512165998870179052969728296241183326209"), u254("26285948001224968191279401605282656404690027600887232916913378662810"), u254("2583483232348049856622220158027381480187355804129549972674415532442")),
            (u254("21888242871839275222246405745257275088512165998870179052969728296241183326209"), u254("4116010325"), u254("21888242871839275222246405745257275088512165998870179052969728296241272804693")),
            (u254("21888242871839275222246405745257275088512165998870179052969728296241183326209"), u254("4187593113"), u254("21888242871839275222246405745257275088512165998870179052969728296241344387481")),
            (u254("21888242871839275222246405745257275088548364400416034343698204186574090508698"), u254("0"), u254("21888242871839275222246405745257275088548364400416034343698204186574090508698")),
            (u254("21888242871839275222246405745257275088548364400416034343698204186574090508698"), u254("1431655765"), u254("21888242871839275222246405745257275088548364400416034343698204186575504268767")),
            (u254("21888242871839275222246405745257275088548364400416034343698204186574090508698"), u254("16704042317112023384305813873602242467992523444124729936442177220570557996694"), u254("2053304182759865277002044103545550374889715499369016654264909112099837960093")),
            (u254("21888242871839275222246405745257275088548364400416034343698204186574090508698"), u254("26285948001224968191279401605282656404690027600887232916913378662810"), u254("2583483232348049856622220158021025763791922282872295785778191636889")),
            (u254("21888242871839275222246405745257275088548364400416034343698204186575629538646"), u254("0"), u254("21888242871839275222246405745257275088548364400416034343698204186575629538646")),
            (u254("21888242871839275222246405745257275088548364400416034343698204186575701121434"), u254("0"), u254("21888242871839275222246405745257275088548364400416034343698204186575701121434")),
            (u254("21888242871839275222246405745257275088548364400416034343698204186575808495616"), u254("0"), u254("21888242871839275222246405745257275088548364400416034343698204186575808495616")),
            (u254("21888242871839275222246405745257275088548364400416034343698204186575808495616"), u254("2147483647"), u254("268435454")),
            (u254("26285948001224968191279401605282656404690027600887232916913378662810"), u254("0"), u254("26285948001224968191279401605282656404690027600887232916913378662810")),
            (u254("26285948001224968191279401605282656404690027600887232916913378662810"), u254("16704042317112023384305813873602242467992523444124729936442177220570557996694"), u254("16704042342551936977365714893961540918536437843192385768022554216171182874526")),
            (u254("26285948001224968191279401605282656404690027600887232916913378662810"), u254("2147483647"), u254("26285948001224968191279401605282656404690027600887232916913754472447")),
            (u254("26285948001224968191279401605282656404690027600887232916913378662810"), u254("21888242871839275222246405745257275088512165998870179052969728296241183326209"), u254("2583483232348049856622220158027381480187355804129549972674415532442")),
            (u254("26285948001224968191279401605282656404690027600887232916913378662810"), u254("21888242871839275222246405745257275088548364400416034343698204186574090508698"), u254("2583483232348049856622220158021025763791922282872295785778191636889")),
            (u254("26285948001224968191279401605282656404690027600887232916913378662810"), u254("4116010325"), u254("26285948001224968191279401605282656404690027600887232916913718681055")),
            (u254("26285948001224968191279401605282656404690027600887232916913378662810"), u254("4187593113"), u254("26285948001224968191279401605282656404690027600887232916913647098267")),
            (u254("4116010325"), u254("0"), u254("4116010325")),
            (u254("4116010325"), u254("4187593113"), u254("4259175901")),
            (u254("4187593113"), u254("0"), u254("4187593113")),
            (u254("7834155660356680066307500141620057836909020373249803374048008616669345161152"), u254("0"), u254("7834155660356680066307500141620057836909020373249803374048008616669345161152")),
            (u254("7834155660356680066307500141620057836909020373249803374048008616669345161152"), u254("16704042317112023384305813873602242467992523444124729936442177220570557996694"), u254("2536648999982979338372617759900832617899818347007980716302112949324769877973")),
        ];
        let ff = &ff_bn256();
        for (a, b, want) in test_cases {
            let result = ff.bor(*a, *b);
            assert_eq!(*want, result, "{} | {} != {}", a, b, want);
        }
    }

    #[test]
    fn test_bn254_lor() {
        let test_cases: &[(U254, U254, U254)] = &[
            (u254("0"), u254("19830999225678642299275043555980204302701349015881432099176924776401679299228"), u254("1")),
            (u254("0"), u254("2"), u254("1")),
            (u254("1"), u254("19830999225678642299275043555980204302701349015881432099176924776401679299228"), u254("1")),
            (u254("1"), u254("2"), u254("1")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("6350874878119819312338956282401532410528162663560392320966563075034087161851"), u254("1")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("7096"), u254("1")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("944936681149208446651664254269745548490766851729442924617792859073125903783"), u254("1")),
            // (u254("115792089237316195423570985008687907853269984665640564039457584007913129639935"), u254("115792089237316195423570985008687907853269984665640564039457584007913129639935"), u254("1")),
            // (u254("115792089237316195423570985008687907853269984665640564039457584007913129639935"), u254("11972743258999954072608883967267172937197689892475318294109741798374968846004"), u254("1")),
            // (u254("115792089237316195423570985008687907853269984665640564039457584007913129639935"), u254("65535"), u254("1")),
            // (u254("11972743258999954072608883967267172937197689892475318294109741798374968846004"), u254("115792089237316195423570985008687907853269984665640564039457584007913129639935"), u254("1")),
            (u254("11972743258999954072608883967267172937197689892475318294109741798374968846004"), u254("11972743258999954072608883967267172937197689892475318294109741798374968846004"), u254("1")),
            (u254("11972743258999954072608883967267172937197689892475318294109741798374968846004"), u254("65535"), u254("1")),
            (u254("16930493065419614647427644856262224012873027146445676318903972992475388670810"), u254("6350874878119819312338956282401532410528162663560392320966563075034087161851"), u254("1")),
            (u254("16930493065419614647427644856262224012873027146445676318903972992475388670810"), u254("7096"), u254("1")),
            (u254("16930493065419614647427644856262224012873027146445676318903972992475388670810"), u254("944936681149208446651664254269745548490766851729442924617792859073125903783"), u254("1")),
            (u254("2147483647"), u254("2147483647"), u254("1")),
            (u254("41456"), u254("6350874878119819312338956282401532410528162663560392320966563075034087161851"), u254("1")),
            (u254("41456"), u254("7096"), u254("1")),
            (u254("41456"), u254("944936681149208446651664254269745548490766851729442924617792859073125903783"), u254("1")),
            // (u254("65535"), u254("115792089237316195423570985008687907853269984665640564039457584007913129639935"), u254("1")),
            (u254("65535"), u254("11972743258999954072608883967267172937197689892475318294109741798374968846004"), u254("1")),
            (u254("9915499612839321149637521777990102151350674507940716049588462388200839649614"), u254("19830999225678642299275043555980204302701349015881432099176924776401679299228"), u254("1")),
            (u254("9915499612839321149637521777990102151350674507940716049588462388200839649614"), u254("2"), u254("1")),
        ];
        let ff = &ff_bn256();
        for (a, b, want) in test_cases {
            let result = ff.lor(*a, *b);
            assert_eq!(*want, result, "{} || {} != {}", a, b, want);
        }
    }

    #[test]
    fn test_bn254_bxor() {
        let test_cases: &[(U254, U254, U254)] = &[
            (u254("0"), u254("0"), u254("0")),
            (u254("0"), u254("1431655765"), u254("1431655765")),
            (u254("0"), u254("16704042317112023384305813873602242467992523444124729936442177220570557996694"), u254("16704042317112023384305813873602242467992523444124729936442177220570557996694")),
            (u254("0"), u254("2147483647"), u254("2147483647")),
            (u254("0"), u254("21888242871839275222246405745257275088512165998870179052969728296241183326209"), u254("21888242871839275222246405745257275088512165998870179052969728296241183326209")),
            (u254("0"), u254("21888242871839275222246405745257275088548364400416034343698204186574090508698"), u254("21888242871839275222246405745257275088548364400416034343698204186574090508698")),
            (u254("0"), u254("21888242871839275222246405745257275088548364400416034343698204186575629538646"), u254("21888242871839275222246405745257275088548364400416034343698204186575629538646")),
            (u254("0"), u254("21888242871839275222246405745257275088548364400416034343698204186575701121434"), u254("21888242871839275222246405745257275088548364400416034343698204186575701121434")),
            (u254("0"), u254("21888242871839275222246405745257275088548364400416034343698204186575808495616"), u254("21888242871839275222246405745257275088548364400416034343698204186575808495616")),
            (u254("0"), u254("26285948001224968191279401605282656404690027600887232916913378662810"), u254("26285948001224968191279401605282656404690027600887232916913378662810")),
            (u254("0"), u254("4116010325"), u254("4116010325")),
            (u254("0"), u254("4187593113"), u254("4187593113")),
            (u254("0"), u254("7834155660356680066307500141620057836909020373249803374048008616669345161152"), u254("7834155660356680066307500141620057836909020373249803374048008616669345161152")),
            (u254("1431655765"), u254("0"), u254("1431655765")),
            (u254("1431655765"), u254("21888242871839275222246405745257275088548364400416034343698204186574090508698"), u254("21888242871839275222246405745257275088548364400416034343698204186575486373071")),
            (u254("16704042317112023384305813873602242467992523444124729936442177220570557996694"), u254("2147483647"), u254("16704042317112023384305813873602242467992523444124729936442177220569764176233")),
            (u254("16704042317112023384305813873602242467992523444124729936442177220570557996694"), u254("21888242871839275222246405745257275088548364400416034343698204186574090508698"), u254("9290808920246982391944680078746133370335271955029337715785845190206644406028")),
            (u254("16704042317112023384305813873602242467992523444124729936442177220570557996694"), u254("26285948001224968191279401605282656404690027600887232916913378662810"), u254("16704042341705902569200647723041437763797695837570013998715698294858429089548")),
            (u254("16704042317112023384305813873602242467992523444124729936442177220570557996694"), u254("7834155660356680066307500141620057836909020373249803374048008616669345161152"), u254("2423342894336530448378327249836640019446457277057462465812244247985445093717")),
            (u254("2147483647"), u254("0"), u254("2147483647")),
            (u254("2147483647"), u254("16704042317112023384305813873602242467992523444124729936442177220570557996694"), u254("16704042317112023384305813873602242467992523444124729936442177220569764176233")),
            (u254("2147483647"), u254("21888242871839275222246405745257275088548364400416034343698204186575808495616"), u254("21888242871839275222246405745257275088548364400416034343698204186574197882879")),
            (u254("2147483647"), u254("26285948001224968191279401605282656404690027600887232916913378662810"), u254("26285948001224968191279401605282656404690027600887232916911982798437")),
            (u254("21888242871839275222246405745257275088512165998870179052969728296241183326209"), u254("0"), u254("21888242871839275222246405745257275088512165998870179052969728296241183326209")),
            (u254("21888242871839275222246405745257275088512165998870179052969728296241183326209"), u254("26285948001224968191279401605282656404690027600887232916913378662810"), u254("21888242850720293685717537267222313799356669357646573641798547105345886067099")),
            (u254("21888242871839275222246405745257275088512165998870179052969728296241183326209"), u254("4116010325"), u254("21888242871839275222246405745257275088512165998870179052969728296237246272852")),
            (u254("21888242871839275222246405745257275088512165998870179052969728296241183326209"), u254("4187593113"), u254("21888242871839275222246405745257275088512165998870179052969728296237317855640")),
            (u254("21888242871839275222246405745257275088548364400416034343698204186574090508698"), u254("0"), u254("21888242871839275222246405745257275088548364400416034343698204186574090508698")),
            (u254("21888242871839275222246405745257275088548364400416034343698204186574090508698"), u254("1431655765"), u254("21888242871839275222246405745257275088548364400416034343698204186575486373071")),
            (u254("21888242871839275222246405745257275088548364400416034343698204186574090508698"), u254("16704042317112023384305813873602242467992523444124729936442177220570557996694"), u254("9290808920246982391944680078746133370335271955029337715785845190206644406028")),
            (u254("21888242871839275222246405745257275088548364400416034343698204186574090508698"), u254("26285948001224968191279401605282656404690027600887232916913378662810"), u254("21888242850720293685717537267222313799307759523309851308555562841220531093504")),
            (u254("21888242871839275222246405745257275088548364400416034343698204186575629538646"), u254("0"), u254("21888242871839275222246405745257275088548364400416034343698204186575629538646")),
            (u254("21888242871839275222246405745257275088548364400416034343698204186575701121434"), u254("0"), u254("21888242871839275222246405745257275088548364400416034343698204186575701121434")),
            (u254("21888242871839275222246405745257275088548364400416034343698204186575808495616"), u254("0"), u254("21888242871839275222246405745257275088548364400416034343698204186575808495616")),
            (u254("21888242871839275222246405745257275088548364400416034343698204186575808495616"), u254("2147483647"), u254("21888242871839275222246405745257275088548364400416034343698204186574197882879")),
            (u254("26285948001224968191279401605282656404690027600887232916913378662810"), u254("0"), u254("26285948001224968191279401605282656404690027600887232916913378662810")),
            (u254("26285948001224968191279401605282656404690027600887232916913378662810"), u254("16704042317112023384305813873602242467992523444124729936442177220570557996694"), u254("16704042341705902569200647723041437763797695837570013998715698294858429089548")),
            (u254("26285948001224968191279401605282656404690027600887232916913378662810"), u254("2147483647"), u254("26285948001224968191279401605282656404690027600887232916911982798437")),
            (u254("26285948001224968191279401605282656404690027600887232916913378662810"), u254("21888242871839275222246405745257275088512165998870179052969728296241183326209"), u254("21888242850720293685717537267222313799356669357646573641798547105345886067099")),
            (u254("26285948001224968191279401605282656404690027600887232916913378662810"), u254("21888242871839275222246405745257275088548364400416034343698204186574090508698"), u254("21888242850720293685717537267222313799307759523309851308555562841220531093504")),
            (u254("26285948001224968191279401605282656404690027600887232916913378662810"), u254("4116010325"), u254("26285948001224968191279401605282656404690027600887232916909942688975")),
            (u254("26285948001224968191279401605282656404690027600887232916913378662810"), u254("4187593113"), u254("26285948001224968191279401605282656404690027600887232916909727940611")),
            (u254("4116010325"), u254("0"), u254("4116010325")),
            (u254("4116010325"), u254("4187593113"), u254("214748364")),
            (u254("4187593113"), u254("0"), u254("4187593113")),
            (u254("7834155660356680066307500141620057836909020373249803374048008616669345161152"), u254("0"), u254("7834155660356680066307500141620057836909020373249803374048008616669345161152")),
            (u254("7834155660356680066307500141620057836909020373249803374048008616669345161152"), u254("16704042317112023384305813873602242467992523444124729936442177220570557996694"), u254("2423342894336530448378327249836640019446457277057462465812244247985445093717")),
        ];
        let ff = &ff_bn256();
        for (a, b, want) in test_cases {
            let result = ff.bxor(*a, *b);
            assert_eq!(*want, result, "{} ^ {} != {}", a, b, want);
        }
    }

    #[test]
    fn test_bn254_idiv() {
        let test_cases: &[(U254, U254, U254)] = &[
            (u254("6"), u254("2"), u254("3")),
            (u254("5"), u254("2"), u254("2")),
        ];
        let ff = &ff_bn256();
        for (a, b, want) in test_cases {
            let result = ff.idiv(*a, *b);
            assert_eq!(*want, result, "{} // {} != {}", a, b, want);
        }
    }

    #[test]
    fn test_bn254_gt() {
        let test_cases: &[(U254, U254, U254)] = &[
            (u254("0"), u254("19830999225678642299275043555980204302701349015881432099176924776401679299228"), u254("1")),
            (u254("0"), u254("2"), u254("0")),
            (u254("1"), u254("19830999225678642299275043555980204302701349015881432099176924776401679299228"), u254("1")),
            (u254("1"), u254("2"), u254("0")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("6350874878119819312338956282401532410528162663560392320966563075034087161851"), u254("1")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("7096"), u254("1")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("944936681149208446651664254269745548490766851729442924617792859073125903783"), u254("1")),
            // (u254("115792089237316195423570985008687907853269984665640564039457584007913129639935"), u254("115792089237316195423570985008687907853269984665640564039457584007913129639935"), u254("0")),
            // (u254("115792089237316195423570985008687907853269984665640564039457584007913129639935"), u254("11972743258999954072608883967267172937197689892475318294109741798374968846004"), u254("1")),
            // (u254("115792089237316195423570985008687907853269984665640564039457584007913129639935"), u254("65535"), u254("0")),
            // (u254("11972743258999954072608883967267172937197689892475318294109741798374968846004"), u254("115792089237316195423570985008687907853269984665640564039457584007913129639935"), u254("0")),
            (u254("11972743258999954072608883967267172937197689892475318294109741798374968846004"), u254("11972743258999954072608883967267172937197689892475318294109741798374968846004"), u254("0")),
            (u254("11972743258999954072608883967267172937197689892475318294109741798374968846004"), u254("65535"), u254("0")),
            (u254("16930493065419614647427644856262224012873027146445676318903972992475388670810"), u254("6350874878119819312338956282401532410528162663560392320966563075034087161851"), u254("0")),
            (u254("16930493065419614647427644856262224012873027146445676318903972992475388670810"), u254("7096"), u254("0")),
            (u254("16930493065419614647427644856262224012873027146445676318903972992475388670810"), u254("944936681149208446651664254269745548490766851729442924617792859073125903783"), u254("0")),
            (u254("2147483647"), u254("2147483647"), u254("0")),
            (u254("41456"), u254("6350874878119819312338956282401532410528162663560392320966563075034087161851"), u254("0")),
            (u254("41456"), u254("7096"), u254("1")),
            (u254("41456"), u254("944936681149208446651664254269745548490766851729442924617792859073125903783"), u254("0")),
            // (u254("65535"), u254("115792089237316195423570985008687907853269984665640564039457584007913129639935"), u254("1")),
            (u254("65535"), u254("11972743258999954072608883967267172937197689892475318294109741798374968846004"), u254("1")),
            (u254("9915499612839321149637521777990102151350674507940716049588462388200839649614"), u254("19830999225678642299275043555980204302701349015881432099176924776401679299228"), u254("1")),
            (u254("9915499612839321149637521777990102151350674507940716049588462388200839649614"), u254("2"), u254("1")),
        ];
        let ff = &ff_bn256();
        for (a, b, want) in test_cases {
            let result = ff.gt(*a, *b);
            assert_eq!(*want, result, "{} > {} != {}", a, b, want);
        }
    }

    #[test]
    fn test_bn254_land() {
        let test_cases: &[(U254, U254, U254)] = &[
            (u254("0"), u254("19830999225678642299275043555980204302701349015881432099176924776401679299228"), u254("0")),
            (u254("0"), u254("2"), u254("0")),
            (u254("1"), u254("19830999225678642299275043555980204302701349015881432099176924776401679299228"), u254("1")),
            (u254("1"), u254("2"), u254("1")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("6350874878119819312338956282401532410528162663560392320966563075034087161851"), u254("1")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("7096"), u254("1")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("944936681149208446651664254269745548490766851729442924617792859073125903783"), u254("1")),
            // (u254("115792089237316195423570985008687907853269984665640564039457584007913129639935"), u254("115792089237316195423570985008687907853269984665640564039457584007913129639935"), u254("1")),
            // (u254("115792089237316195423570985008687907853269984665640564039457584007913129639935"), u254("11972743258999954072608883967267172937197689892475318294109741798374968846004"), u254("1")),
            // (u254("115792089237316195423570985008687907853269984665640564039457584007913129639935"), u254("65535"), u254("1")),
            // (u254("11972743258999954072608883967267172937197689892475318294109741798374968846004"), u254("115792089237316195423570985008687907853269984665640564039457584007913129639935"), u254("1")),
            (u254("11972743258999954072608883967267172937197689892475318294109741798374968846004"), u254("11972743258999954072608883967267172937197689892475318294109741798374968846004"), u254("1")),
            (u254("11972743258999954072608883967267172937197689892475318294109741798374968846004"), u254("65535"), u254("1")),
            (u254("16930493065419614647427644856262224012873027146445676318903972992475388670810"), u254("6350874878119819312338956282401532410528162663560392320966563075034087161851"), u254("1")),
            (u254("16930493065419614647427644856262224012873027146445676318903972992475388670810"), u254("7096"), u254("1")),
            (u254("16930493065419614647427644856262224012873027146445676318903972992475388670810"), u254("944936681149208446651664254269745548490766851729442924617792859073125903783"), u254("1")),
            (u254("2147483647"), u254("2147483647"), u254("1")),
            (u254("41456"), u254("6350874878119819312338956282401532410528162663560392320966563075034087161851"), u254("1")),
            (u254("41456"), u254("7096"), u254("1")),
            (u254("41456"), u254("944936681149208446651664254269745548490766851729442924617792859073125903783"), u254("1")),
            // (u254("65535"), u254("115792089237316195423570985008687907853269984665640564039457584007913129639935"), u254("1")),
            (u254("65535"), u254("11972743258999954072608883967267172937197689892475318294109741798374968846004"), u254("1")),
            (u254("9915499612839321149637521777990102151350674507940716049588462388200839649614"), u254("19830999225678642299275043555980204302701349015881432099176924776401679299228"), u254("1")),
            (u254("9915499612839321149637521777990102151350674507940716049588462388200839649614"), u254("2"), u254("1")),
        ];
        let ff = &ff_bn256();
        for (a, b, want) in test_cases {
            let result = ff.land(*a, *b);
            assert_eq!(*want, result, "{} && {} != {}", a, b, want);
        }
    }

    #[test]
    fn test_bn254_gte() {
        let test_cases: &[(U254, U254, U254)] = &[
            (u254("0"), u254("19830999225678642299275043555980204302701349015881432099176924776401679299228"), u254("1")),
            (u254("0"), u254("2"), u254("0")),
            (u254("1"), u254("19830999225678642299275043555980204302701349015881432099176924776401679299228"), u254("1")),
            (u254("1"), u254("2"), u254("0")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("6350874878119819312338956282401532410528162663560392320966563075034087161851"), u254("1")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("7096"), u254("1")),
            (u254("10944121435919637611123202872628637544274182200208017171849102093287904247808"), u254("944936681149208446651664254269745548490766851729442924617792859073125903783"), u254("1")),
            // (u254("115792089237316195423570985008687907853269984665640564039457584007913129639935"), u254("115792089237316195423570985008687907853269984665640564039457584007913129639935"), u254("1")),
            // (u254("115792089237316195423570985008687907853269984665640564039457584007913129639935"), u254("11972743258999954072608883967267172937197689892475318294109741798374968846004"), u254("1")),
            // (u254("115792089237316195423570985008687907853269984665640564039457584007913129639935"), u254("65535"), u254("0")),
            // (u254("11972743258999954072608883967267172937197689892475318294109741798374968846004"), u254("115792089237316195423570985008687907853269984665640564039457584007913129639935"), u254("0")),
            (u254("11972743258999954072608883967267172937197689892475318294109741798374968846004"), u254("11972743258999954072608883967267172937197689892475318294109741798374968846004"), u254("1")),
            (u254("11972743258999954072608883967267172937197689892475318294109741798374968846004"), u254("65535"), u254("0")),
            (u254("16930493065419614647427644856262224012873027146445676318903972992475388670810"), u254("6350874878119819312338956282401532410528162663560392320966563075034087161851"), u254("0")),
            (u254("16930493065419614647427644856262224012873027146445676318903972992475388670810"), u254("7096"), u254("0")),
            (u254("16930493065419614647427644856262224012873027146445676318903972992475388670810"), u254("944936681149208446651664254269745548490766851729442924617792859073125903783"), u254("0")),
            (u254("2147483647"), u254("2147483647"), u254("1")),
            (u254("41456"), u254("6350874878119819312338956282401532410528162663560392320966563075034087161851"), u254("0")),
            (u254("41456"), u254("7096"), u254("1")),
            (u254("41456"), u254("944936681149208446651664254269745548490766851729442924617792859073125903783"), u254("0")),
            // (u254("65535"), u254("115792089237316195423570985008687907853269984665640564039457584007913129639935"), u254("1")),
            (u254("65535"), u254("11972743258999954072608883967267172937197689892475318294109741798374968846004"), u254("1")),
            (u254("9915499612839321149637521777990102151350674507940716049588462388200839649614"), u254("19830999225678642299275043555980204302701349015881432099176924776401679299228"), u254("1")),
            (u254("9915499612839321149637521777990102151350674507940716049588462388200839649614"), u254("2"), u254("1")),
        ];
        let ff = &ff_bn256();
        for (a, b, want) in test_cases {
            let result = ff.gte(*a, *b);
            assert_eq!(*want, result, "{} >= {} != {}", a, b, want);
        }
    }

    #[test]
    fn test_bn254_lte() {
        let test_cases: &[(U254, U254, U254)] = &[
            (u254("1"), u254("2"), u254("1")),
            (u254("0"), u254("2"), u254("1")),
            (u254("41456"), u254("944936681149208446651664254269745548490766851729442924617792859073125903783"), u254("1")),
            // (u254("65535"), u254("115792089237316195423570985008687907853269984665640564039457584007913129639935"), u254("0")),
        ];
        let ff = &ff_bn256();
        for (a, b, want) in test_cases {
            let result = ff.lte(*a, *b);
            assert_eq!(*want, result, "{} <= {} != {}", a, b, want);
        }
    }

    #[test]
    fn test_bytes_le_u64() {
        let bs = [1u8];
        let x = U64::from_le_bytes(&bs).unwrap();
        assert_eq!(x.0, 1u64);
    }

    #[test]
    fn test_bytes_le_u254() {
        let bs = [1u8];
        let x = <U254 as FieldOps>::from_le_bytes(&bs).unwrap();
        assert_eq!(x, U254::one());
    }

    #[test]
    fn test_field_parse_str() {
        let ff = Field::new(bn254_prime);

        let x = (&ff).parse_str("3").unwrap();
        assert_eq!(x, U254::from_str("3").unwrap());

        let x = (&ff).parse_str("21888242871839275222246405745257275088548364400416034343698204186575808495617").unwrap();
        assert_eq!(x, U254::from_str("0").unwrap());

        let x = (&ff).parse_str("21888242871839275222246405745257275088548364400416034343698204186575808495618").unwrap();
        assert_eq!(x, U254::from_str("1").unwrap());

        let x = (&ff).parse_str("-1").unwrap();
        assert_eq!(x, U254::from_str("21888242871839275222246405745257275088548364400416034343698204186575808495616").unwrap());

        let x = (&ff).parse_str("-2").unwrap();
        assert_eq!(x, U254::from_str("21888242871839275222246405745257275088548364400416034343698204186575808495615").unwrap());
    }
}

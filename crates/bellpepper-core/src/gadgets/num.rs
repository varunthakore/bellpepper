//! Gadgets representing numbers in the scalar field of the underlying curve.

use ff::{PrimeField, PrimeFieldBits};
use serde::{Deserialize, Serialize};

use crate::{ConstraintSystem, LinearCombination, SynthesisError, Variable};

use crate::gadgets::boolean::{self, AllocatedBit, Boolean};

#[derive(Debug, Copy, Serialize, Deserialize)]
pub struct AllocatedNum<Scalar: PrimeField> {
    value: Option<Scalar>,
    variable: Variable,
}

impl<Scalar: PrimeField> Clone for AllocatedNum<Scalar> {
    fn clone(&self) -> Self {
        AllocatedNum {
            value: self.value,
            variable: self.variable,
        }
    }
}

impl<Scalar: PrimeField> AllocatedNum<Scalar> {
    /// Allocate a `Variable(Aux)` in a `ConstraintSystem`.
    pub fn alloc<CS, F>(mut cs: CS, value: F) -> Result<Self, SynthesisError>
    where
        CS: ConstraintSystem<Scalar>,
        F: FnOnce() -> Result<Scalar, SynthesisError>,
    {
        let mut new_value = None;
        let var = cs.alloc(
            || "num",
            || {
                let tmp = value()?;

                new_value = Some(tmp);

                Ok(tmp)
            },
        )?;

        Ok(AllocatedNum {
            value: new_value,
            variable: var,
        })
    }

    /// Allocate a `Variable(Aux)` in a `ConstraintSystem`. Requires an
    /// infallible getter for the value.
    pub fn alloc_infallible<CS, F>(cs: CS, value: F) -> Self
    where
        CS: ConstraintSystem<Scalar>,
        F: FnOnce() -> Scalar,
    {
        Self::alloc(cs, || Ok(value())).unwrap()
    }

    /// Allocate a `Variable(Input)` in a `ConstraintSystem`.
    pub fn alloc_input<CS, F>(mut cs: CS, value: F) -> Result<Self, SynthesisError>
    where
        CS: ConstraintSystem<Scalar>,
        F: FnOnce() -> Result<Scalar, SynthesisError>,
    {
        let mut new_value = None;
        let var = cs.alloc_input(
            || "input num",
            || {
                let tmp = value()?;

                new_value = Some(tmp);

                Ok(tmp)
            },
        )?;

        Ok(AllocatedNum {
            value: new_value,
            variable: var,
        })
    }

    /// Allocate a `Variable` of either `Aux` or `Input` in a
    /// `ConstraintSystem`. The `Variable` is a an `Input` if `is_input` is
    /// true. This allows uniform creation of circuits containing components
    /// which may or may not be public inputs.
    pub fn alloc_maybe_input<CS, F>(
        cs: CS,
        is_input: bool,
        value: F,
    ) -> Result<Self, SynthesisError>
    where
        CS: ConstraintSystem<Scalar>,
        F: FnOnce() -> Result<Scalar, SynthesisError>,
    {
        if is_input {
            Self::alloc_input(cs, value)
        } else {
            Self::alloc(cs, value)
        }
    }

    pub fn inputize<CS>(&self, mut cs: CS) -> Result<(), SynthesisError>
    where
        CS: ConstraintSystem<Scalar>,
    {
        let input = cs.alloc_input(
            || "input variable",
            || self.value.ok_or(SynthesisError::AssignmentMissing),
        )?;

        cs.enforce(
            || "enforce input is correct",
            |lc| lc + input,
            |lc| lc + CS::one(),
            |lc| lc + self.variable,
        );

        Ok(())
    }

    /// Deconstructs this allocated number into its
    /// boolean representation in little-endian bit
    /// order, requiring that the representation
    /// strictly exists "in the field" (i.e., a
    /// congruency is not allowed.)
    pub fn to_bits_le_strict<CS>(&self, mut cs: CS) -> Result<Vec<Boolean>, SynthesisError>
    where
        CS: ConstraintSystem<Scalar>,
        Scalar: PrimeFieldBits,
    {
        pub fn kary_and<Scalar, CS>(
            mut cs: CS,
            v: &[AllocatedBit],
        ) -> Result<AllocatedBit, SynthesisError>
        where
            Scalar: PrimeField,
            CS: ConstraintSystem<Scalar>,
        {
            assert!(!v.is_empty());

            // Let's keep this simple for now and just AND them all
            // manually
            let mut cur = None;

            for (i, v) in v.iter().enumerate() {
                if cur.is_none() {
                    cur = Some(v.clone());
                } else {
                    cur = Some(AllocatedBit::and(
                        cs.namespace(|| format!("and {}", i)),
                        cur.as_ref().unwrap(),
                        v,
                    )?);
                }
            }

            Ok(cur.expect("v.len() > 0"))
        }

        // We want to ensure that the bit representation of a is
        // less than or equal to r - 1.
        let a = self.value.map(|e| e.to_le_bits());
        let b = (-Scalar::ONE).to_le_bits();

        // Get the bits of `a` in big-endian order.
        let mut a = a.as_ref().map(|e| e.into_iter().rev());

        let mut result = vec![];

        // Runs of ones in r
        let mut last_run = None;
        let mut current_run = vec![];

        let mut found_one = false;
        let mut i = 0;
        for b in b.into_iter().rev() {
            let a_bit: Option<bool> = a.as_mut().map(|e| *e.next().unwrap());

            // Skip over unset bits at the beginning
            found_one |= b;
            if !found_one {
                // a_bit should also be false
                if let Some(a_bit) = a_bit {
                    assert!(!a_bit);
                }
                continue;
            }

            if b {
                // This is part of a run of ones. Let's just
                // allocate the boolean with the expected value.
                let a_bit = AllocatedBit::alloc(cs.namespace(|| format!("bit {}", i)), a_bit)?;
                // ... and add it to the current run of ones.
                current_run.push(a_bit.clone());
                result.push(a_bit);
            } else {
                if !current_run.is_empty() {
                    // This is the start of a run of zeros, but we need
                    // to k-ary AND against `last_run` first.

                    if last_run.is_some() {
                        current_run.push(last_run.clone().unwrap());
                    }
                    last_run = Some(kary_and(
                        cs.namespace(|| format!("run ending at {}", i)),
                        &current_run,
                    )?);
                    current_run.truncate(0);
                }

                // If `last_run` is true, `a` must be false, or it would
                // not be in the field.
                //
                // If `last_run` is false, `a` can be true or false.

                let a_bit = AllocatedBit::alloc_conditionally(
                    cs.namespace(|| format!("bit {}", i)),
                    a_bit,
                    last_run.as_ref().expect("char always starts with a one"),
                )?;
                result.push(a_bit);
            }

            i += 1;
        }

        // char is prime, so we'll always end on
        // a run of zeros.
        assert_eq!(current_run.len(), 0);

        // Now, we have `result` in big-endian order.
        // However, now we have to unpack self!

        let mut lc = LinearCombination::zero();
        let mut coeff = Scalar::ONE;

        for bit in result.iter().rev() {
            lc = lc + (coeff, bit.get_variable());

            coeff = coeff.double();
        }

        lc = lc - self.variable;

        cs.enforce(|| "unpacking constraint", |lc| lc, |lc| lc, |_| lc);

        // Convert into booleans, and reverse for little-endian bit order
        Ok(result.into_iter().map(Boolean::from).rev().collect())
    }

    /// Convert the allocated number into its little-endian representation.
    /// Note that this does not strongly enforce that the commitment is
    /// "in the field."
    pub fn to_bits_le<CS>(&self, mut cs: CS) -> Result<Vec<Boolean>, SynthesisError>
    where
        CS: ConstraintSystem<Scalar>,
        Scalar: PrimeFieldBits,
    {
        let bits = boolean::field_into_allocated_bits_le(&mut cs, self.value)?;

        let mut lc = LinearCombination::zero();
        let mut coeff = Scalar::ONE;

        for bit in bits.iter() {
            lc = lc + (coeff, bit.get_variable());

            coeff = coeff.double();
        }

        lc = lc - self.variable;

        cs.enforce(|| "unpacking constraint", |lc| lc, |lc| lc, |_| lc);

        Ok(bits.into_iter().map(Boolean::from).collect())
    }

    pub fn add<CS>(&self, mut cs: CS, other: &Self) -> Result<Self, SynthesisError>
    where
        CS: ConstraintSystem<Scalar>,
    {
        let mut value = None;

        let var = cs.alloc(
            || "sum num",
            || {
                let mut tmp = self.value.ok_or(SynthesisError::AssignmentMissing)?;
                tmp.add_assign(other.value.ok_or(SynthesisError::AssignmentMissing)?);

                value = Some(tmp);

                Ok(tmp)
            },
        )?;

        // Constrain: (a + b) * 1 = a + b
        cs.enforce(
            || "addition constraint",
            |lc| lc + self.variable + other.variable,
            |lc| lc + CS::one(),
            |lc| lc + var,
        );

        Ok(AllocatedNum {
            value,
            variable: var,
        })
    }

    /// Returns (self - other)
    pub fn sub<CS>(&self, mut cs: CS, other: &Self) -> Result<Self, SynthesisError>
    where
        CS: ConstraintSystem<Scalar>,
    {
        let mut value = None;

        let var = cs.alloc(
            || "sub num",
            || {
                let mut tmp = self.value.ok_or(SynthesisError::AssignmentMissing)?;
                tmp.sub_assign(other.value.ok_or(SynthesisError::AssignmentMissing)?);

                value = Some(tmp);

                Ok(tmp)
            },
        )?;

        // Constrain: (a - b) * 1 = a - b
        cs.enforce(
            || "subtraction constraint",
            |lc| lc + self.variable - other.variable,
            |lc| lc + CS::one(),
            |lc| lc + var,
        );

        Ok(AllocatedNum {
            value,
            variable: var,
        })
    }

    /// Returns (-self)
    pub fn neg<CS>(&self, mut cs: CS) -> Result<Self, SynthesisError>
    where
        CS: ConstraintSystem<Scalar>,
    {
        let mut value = None;

        let var = cs.alloc(
            || "neg num",
            || {
                let tmp = self.value.ok_or(SynthesisError::AssignmentMissing)?.neg();

                value = Some(tmp);

                Ok(tmp)
            },
        )?;

        // Constrain: (self + var) = 0
        cs.enforce(
            || "negation constraint",
            |lc| lc,
            |lc| lc,
            |lc| lc + self.variable + var,
        );

        Ok(AllocatedNum {
            value,
            variable: var,
        })
    }

    pub fn mul<CS>(&self, mut cs: CS, other: &Self) -> Result<Self, SynthesisError>
    where
        CS: ConstraintSystem<Scalar>,
    {
        let mut value = None;

        let var = cs.alloc(
            || "product num",
            || {
                let mut tmp = self.value.ok_or(SynthesisError::AssignmentMissing)?;
                tmp.mul_assign(other.value.ok_or(SynthesisError::AssignmentMissing)?);

                value = Some(tmp);

                Ok(tmp)
            },
        )?;

        // Constrain: a * b = ab
        cs.enforce(
            || "multiplication constraint",
            |lc| lc + self.variable,
            |lc| lc + other.variable,
            |lc| lc + var,
        );

        Ok(AllocatedNum {
            value,
            variable: var,
        })
    }

    /// Returns (self)*(other)^-1
    pub fn div<CS>(&self, mut cs: CS, other: &Self) -> Result<Self, SynthesisError>
    where
        CS: ConstraintSystem<Scalar>,
    {
        let mut value = None;

        let var = cs.alloc(
            || "div num",
            || {
                let mut tmp = self.value.ok_or(SynthesisError::AssignmentMissing)?;
                let other_inv = other
                    .value
                    .ok_or(SynthesisError::AssignmentMissing)?
                    .invert();
                assert!(other_inv.is_some().unwrap_u8() == 1);
                let other_inv = other_inv.unwrap();
                tmp.mul_assign(other_inv);

                value = Some(tmp);

                Ok(tmp)
            },
        )?;

        // Constrain: var * other = self
        cs.enforce(
            || "division constraint",
            |lc| lc + var,
            |lc| lc + other.variable,
            |lc| lc + self.variable,
        );

        Ok(AllocatedNum {
            value,
            variable: var,
        })
    }

    pub fn square<CS>(&self, mut cs: CS) -> Result<Self, SynthesisError>
    where
        CS: ConstraintSystem<Scalar>,
    {
        let mut value = None;

        let var = cs.alloc(
            || "squared num",
            || {
                let mut tmp = self.value.ok_or(SynthesisError::AssignmentMissing)?;
                tmp = tmp.square();

                value = Some(tmp);

                Ok(tmp)
            },
        )?;

        // Constrain: a * a = aa
        cs.enforce(
            || "squaring constraint",
            |lc| lc + self.variable,
            |lc| lc + self.variable,
            |lc| lc + var,
        );

        Ok(AllocatedNum {
            value,
            variable: var,
        })
    }

    pub fn assert_nonzero<CS>(&self, mut cs: CS) -> Result<(), SynthesisError>
    where
        CS: ConstraintSystem<Scalar>,
    {
        let inv = cs.alloc(
            || "ephemeral inverse",
            || {
                let tmp = self.value.ok_or(SynthesisError::AssignmentMissing)?;

                if tmp.is_zero().into() {
                    Err(SynthesisError::DivisionByZero)
                } else {
                    Ok(tmp.invert().unwrap())
                }
            },
        )?;

        // Constrain a * inv = 1, which is only valid
        // iff a has a multiplicative inverse, untrue
        // for zero.
        cs.enforce(
            || "nonzero assertion constraint",
            |lc| lc + self.variable,
            |lc| lc + inv,
            |lc| lc + CS::one(),
        );

        Ok(())
    }

    /// Returns the bit `self == 0`
    pub fn is_zero<CS>(&self, mut cs: CS) -> Result<Boolean, SynthesisError>
    where
        CS: ConstraintSystem<Scalar>,
    {
        let out = AllocatedBit::alloc(&mut cs.namespace(|| "out bit"), {
            let input_value = self.value.ok_or(SynthesisError::AssignmentMissing)?;
            Some(input_value == Scalar::ZERO)
        })?;
        let multiplier = Self::alloc(&mut cs.namespace(|| "zero or inverse"), || {
            let tmp = self.value.ok_or(SynthesisError::AssignmentMissing)?;

            if tmp.is_zero().into() {
                Ok(Scalar::ZERO)
            } else {
                Ok(tmp.invert().unwrap())
            }
        })?;

        cs.enforce(
            || "multiplier * input === 1 - out",
            |lc| lc + multiplier.variable,
            |lc| lc + self.variable,
            |lc| lc + CS::one() - out.get_variable(),
        );

        cs.enforce(
            || "out * input === 0",
            |lc| lc + out.get_variable(),
            |lc| lc + self.variable,
            |lc| lc,
        );
        Ok(Boolean::from(out))
    }

    /// Takes two allocated numbers (self, other) and returns
    /// the bit `self==other`
    pub fn is_equal<CS>(&self, mut cs: CS, other: &Self) -> Result<Boolean, SynthesisError>
    where
        CS: ConstraintSystem<Scalar>,
    {
        let diff = self.sub(&mut cs.namespace(|| "self-other"), other)?;
        Self::is_zero(&diff, cs)
    }

    /// Takes two allocated numbers (a, b) and returns
    /// a if condition is false, and b otherwise
    pub fn conditionally_select<CS>(
        mut cs: CS,
        a: &Self,
        b: &Self,
        condition: &Boolean,
    ) -> Result<Self, SynthesisError>
    where
        CS: ConstraintSystem<Scalar>,
    {
        let c = Self::alloc(&mut cs.namespace(|| "alloc output"), || {
            if condition
                .get_value()
                .ok_or(SynthesisError::AssignmentMissing)?
            {
                Ok(b.value.ok_or(SynthesisError::AssignmentMissing)?)
            } else {
                Ok(a.value.ok_or(SynthesisError::AssignmentMissing)?)
            }
        })?;
        cs.enforce(
            || "condition * (a - b) === a - c",
            |_| condition.lc(CS::one(), Scalar::ONE),
            |lc| lc + a.variable - b.variable,
            |lc| lc + a.variable - c.variable,
        );

        Ok(c)
    }

    /// Takes two allocated numbers (a, b) and returns
    /// (b, a) if the condition is true, and (a, b)
    /// otherwise.
    pub fn conditionally_reverse<CS>(
        mut cs: CS,
        a: &Self,
        b: &Self,
        condition: &Boolean,
    ) -> Result<(Self, Self), SynthesisError>
    where
        CS: ConstraintSystem<Scalar>,
    {
        let c = Self::alloc(cs.namespace(|| "conditional reversal result 1"), || {
            if condition
                .get_value()
                .ok_or(SynthesisError::AssignmentMissing)?
            {
                Ok(b.value.ok_or(SynthesisError::AssignmentMissing)?)
            } else {
                Ok(a.value.ok_or(SynthesisError::AssignmentMissing)?)
            }
        })?;

        cs.enforce(
            || "first conditional reversal",
            |lc| lc + a.variable - b.variable,
            |_| condition.lc(CS::one(), Scalar::ONE),
            |lc| lc + a.variable - c.variable,
        );

        let d = Self::alloc(cs.namespace(|| "conditional reversal result 2"), || {
            if condition
                .get_value()
                .ok_or(SynthesisError::AssignmentMissing)?
            {
                Ok(a.value.ok_or(SynthesisError::AssignmentMissing)?)
            } else {
                Ok(b.value.ok_or(SynthesisError::AssignmentMissing)?)
            }
        })?;

        cs.enforce(
            || "second conditional reversal",
            |lc| lc + b.variable - a.variable,
            |_| condition.lc(CS::one(), Scalar::ONE),
            |lc| lc + b.variable - d.variable,
        );

        Ok((c, d))
    }

    /// Builds a mux tree. The first bit is taken as the highest order.
    // Code Adapted from https://github.com/alex-ozdemir/bellman-bignat/blob/0e10f9f7ef4a061deaf4d7684d398dca613174c8/src/util/gadget.rs#L124
    pub fn mux_tree<'a, CS>(
        cs: &mut CS,
        mut select_bits: impl Iterator<Item = &'a Boolean> + Clone,
        inputs: &[Self],
    ) -> Result<Self, SynthesisError>
    where
        CS: ConstraintSystem<Scalar>,
    {
        if let Some(bit) = select_bits.next() {
            if inputs.len() & 1 != 0 {
                return Err(SynthesisError::Unsatisfiable);
            }
            let left_half = &inputs[..(inputs.len() / 2)];
            let right_half = &inputs[(inputs.len() / 2)..];
            let left =
                Self::mux_tree(&mut cs.namespace(|| "left"), select_bits.clone(), left_half)?;
            let right = Self::mux_tree(&mut cs.namespace(|| "right"), select_bits, right_half)?;
            Self::conditionally_select(&mut cs.namespace(|| "join"), &left, &right, bit)
        } else {
            if inputs.len() != 1 {
                return Err(SynthesisError::Unsatisfiable);
            }
            Ok(inputs[0].clone())
        }
    }

    pub fn get_value(&self) -> Option<Scalar> {
        self.value
    }

    pub fn get_variable(&self) -> Variable {
        self.variable
    }
}

#[derive(Debug, Clone)]
pub struct Num<Scalar: PrimeField> {
    value: Option<Scalar>,
    lc: LinearCombination<Scalar>,
}

impl<Scalar: PrimeField> From<AllocatedNum<Scalar>> for Num<Scalar> {
    fn from(num: AllocatedNum<Scalar>) -> Num<Scalar> {
        Num {
            value: num.value,
            lc: LinearCombination::<Scalar>::from_variable(num.variable),
        }
    }
}

impl<Scalar: PrimeField> Num<Scalar> {
    pub fn zero() -> Self {
        Num {
            value: Some(Scalar::ZERO),
            lc: LinearCombination::zero(),
        }
    }

    pub fn get_value(&self) -> Option<Scalar> {
        self.value
    }

    pub fn lc(&self, coeff: Scalar) -> LinearCombination<Scalar> {
        LinearCombination::zero() + (coeff, &self.lc)
    }

    pub fn add_bool_with_coeff(self, one: Variable, bit: &Boolean, coeff: Scalar) -> Self {
        let newval = match (self.value, bit.get_value()) {
            (Some(mut curval), Some(bval)) => {
                if bval {
                    curval.add_assign(&coeff);
                }

                Some(curval)
            }
            _ => None,
        };

        Num {
            value: newval,
            lc: self.lc + &bit.lc(one, coeff),
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn add(self, other: &Self) -> Self {
        let lc = self.lc + &other.lc;
        let value = match (self.value, other.value) {
            (Some(v1), Some(v2)) => {
                let mut tmp = v1;
                tmp.add_assign(&v2);
                Some(tmp)
            }
            _ => None,
        };

        Num { value, lc }
    }

    pub fn scale(mut self, scalar: Scalar) -> Self {
        for (_variable, fr) in self.lc.iter_mut() {
            fr.mul_assign(&scalar);
        }

        if let Some(ref mut v) = self.value {
            v.mul_assign(&scalar);
        }

        self
    }
}

#[cfg(test)]
mod test {
    use std::ops::{AddAssign, MulAssign, Neg, SubAssign};

    use crate::ConstraintSystem;
    use blstrs::Scalar as Fr;
    use ff::{Field, PrimeField, PrimeFieldBits};
    use rand_core::SeedableRng;
    use rand_xorshift::XorShiftRng;

    use super::{AllocatedNum, Boolean, Num};
    use crate::util_cs::test_cs::*;

    #[test]
    fn test_allocated_num() {
        let mut cs = TestConstraintSystem::<Fr>::new();

        AllocatedNum::alloc(&mut cs, || Ok(Fr::ONE)).unwrap();

        assert!(cs.get("num") == Fr::ONE);
    }

    #[test]
    fn test_allocated_infallible_num() {
        let mut cs = TestConstraintSystem::<Fr>::new();

        AllocatedNum::alloc_infallible(&mut cs, || Fr::ONE);

        assert!(cs.get("num") == Fr::ONE);
    }

    #[test]
    fn test_num_partial_addition() {
        let a = Num::<Fr>::zero();
        let b = Num::<Fr> {
            value: None,
            lc: Default::default(),
        };
        let c = a.clone().add(&b);
        assert!(c.value.is_none());
        let c = b.clone().add(&a);
        assert!(c.value.is_none());
        let c = b.clone().add(&b);
        assert!(c.value.is_none());
        let c = a.clone().add(&a);
        assert!(c.value == Some(Fr::ZERO));
    }

    #[test]
    fn test_num_addition() {
        let mut cs = TestConstraintSystem::<Fr>::new();

        let mut char = Fr::char();
        char[0] -= 1u8;
        let mod_minus_one = Fr::from_repr(char);
        assert!(bool::from(mod_minus_one.is_some()));
        let mod_minus_one = mod_minus_one.unwrap();

        let a = AllocatedNum::alloc(cs.namespace(|| "a"), || Ok(mod_minus_one)).unwrap();
        let b = AllocatedNum::alloc(cs.namespace(|| "b"), || Ok(Fr::ONE)).unwrap();
        let c = a.add(&mut cs, &b).unwrap();

        assert!(cs.is_satisfied());
        assert!(cs.get("sum num") == Fr::ZERO);
        assert!(c.value.unwrap() == Fr::ZERO);
        cs.set("sum num", Fr::ONE);
        assert!(!cs.is_satisfied());
    }

    #[test]
    fn test_num_subraction() {
        let mut cs = TestConstraintSystem::<Fr>::new();

        let mut char = Fr::char();
        char[0] -= 1u8;
        let mod_minus_one = Fr::from_repr(char);
        assert!(bool::from(mod_minus_one.is_some()));
        let mod_minus_one = mod_minus_one.unwrap();

        let a = AllocatedNum::alloc(cs.namespace(|| "a"), || Ok(Fr::ZERO)).unwrap();
        let b = AllocatedNum::alloc(cs.namespace(|| "b"), || Ok(Fr::ONE)).unwrap();
        let c = a.sub(&mut cs, &b).unwrap();

        assert!(cs.is_satisfied());
        assert!(cs.get("sub num") == mod_minus_one);
        assert!(c.value.unwrap() == mod_minus_one);
        cs.set("sub num", Fr::ONE);
        assert!(!cs.is_satisfied());
    }

    #[test]
    fn test_num_negation() {
        let mut rng = XorShiftRng::from_seed([
            0x59, 0x62, 0xbe, 0x3d, 0x76, 0x3d, 0x31, 0x8d, 0x17, 0xdb, 0x37, 0x32, 0x54, 0x06,
            0xbc, 0xe5,
        ]);
        let mut cs = TestConstraintSystem::<Fr>::new();

        let a = AllocatedNum::alloc(cs.namespace(|| "a"), || Ok(Fr::random(&mut rng))).unwrap();
        let out = a.neg(&mut cs).unwrap();
        let out_exp = a.get_value().unwrap().neg();

        assert!(cs.is_satisfied());
        assert!(cs.get("neg num") == out_exp);
        assert!(out.value.unwrap() == out_exp);
        cs.set("neg num", Fr::random(&mut rng));
        assert!(!cs.is_satisfied());
    }

    #[test]
    fn test_num_squaring() {
        let mut cs = TestConstraintSystem::<Fr>::new();

        let n = AllocatedNum::alloc(&mut cs, || Ok(Fr::from(3u64))).unwrap();
        let n2 = n.square(&mut cs).unwrap();

        assert!(cs.is_satisfied());
        assert!(cs.get("squared num") == Fr::from(9u64));
        assert!(n2.value.unwrap() == Fr::from(9u64));
        cs.set("squared num", Fr::from(10u64));
        assert!(!cs.is_satisfied());
    }

    #[test]
    fn test_num_multiplication() {
        let mut cs = TestConstraintSystem::<Fr>::new();

        let n = AllocatedNum::alloc(cs.namespace(|| "a"), || Ok(Fr::from(12u64))).unwrap();
        let n2 = AllocatedNum::alloc(cs.namespace(|| "b"), || Ok(Fr::from(10u64))).unwrap();
        let n3 = n.mul(&mut cs, &n2).unwrap();

        assert!(cs.is_satisfied());
        assert!(cs.get("product num") == Fr::from(120u64));
        assert!(n3.value.unwrap() == Fr::from(120u64));
        cs.set("product num", Fr::from(121u64));
        assert!(!cs.is_satisfied());
    }

    #[test]
    fn test_num_division() {
        let mut cs = TestConstraintSystem::<Fr>::new();

        let a = AllocatedNum::alloc(cs.namespace(|| "a"), || Ok(Fr::from(120u64))).unwrap();
        let b = AllocatedNum::alloc(cs.namespace(|| "b"), || Ok(Fr::from(10u64))).unwrap();
        let c = a.div(&mut cs, &b).unwrap();

        assert!(cs.is_satisfied());
        assert!(cs.get("div num") == Fr::from(12u64));
        assert!(c.value.unwrap() == Fr::from(12u64));
        cs.set("div num", Fr::from(11u64));
        assert!(!cs.is_satisfied());
    }

    #[test]
    fn test_num_is_zero() {
        let mut rng = XorShiftRng::from_seed([
            0x59, 0x62, 0xbe, 0x3d, 0x76, 0x3d, 0x31, 0x8d, 0x17, 0xdb, 0x37, 0x32, 0x54, 0x06,
            0xbc, 0xe5,
        ]);
        {
            let mut cs = TestConstraintSystem::<Fr>::new();
            let a = AllocatedNum::alloc(cs.namespace(|| "a"), || Ok(Fr::random(&mut rng))).unwrap();
            let is_zero = a.is_zero(&mut cs).unwrap();

            assert!(cs.is_satisfied());
            assert!(cs.get("out bit/boolean") == Fr::from(0u64));
            assert!(!is_zero.get_value().unwrap());
            cs.set("out bit/boolean", Fr::from(1u64));
            assert!(!cs.is_satisfied());
        }

        {
            let mut cs = TestConstraintSystem::<Fr>::new();
            let a = AllocatedNum::alloc(cs.namespace(|| "a"), || Ok(Fr::ZERO)).unwrap();
            let is_zero = a.is_zero(&mut cs).unwrap();
            assert!(cs.is_satisfied());
            assert!(cs.get("out bit/boolean") == Fr::from(1u64));
            assert!(is_zero.get_value().unwrap());
            cs.set("out bit/boolean", Fr::from(0u64));
            assert!(!cs.is_satisfied());
        }
    }

    #[test]
    fn test_num_is_equal() {
        let mut rng = XorShiftRng::from_seed([
            0x59, 0x62, 0xbe, 0x3d, 0x76, 0x3d, 0x31, 0x8d, 0x17, 0xdb, 0x37, 0x32, 0x54, 0x06,
            0xbc, 0xe5,
        ]);
        {
            let mut cs = TestConstraintSystem::<Fr>::new();
            let a = AllocatedNum::alloc(cs.namespace(|| "a"), || Ok(Fr::random(&mut rng))).unwrap();
            let b = AllocatedNum::alloc(cs.namespace(|| "b"), || Ok(Fr::random(&mut rng))).unwrap();
            let is_equal = a.is_equal(&mut cs, &b).unwrap();

            assert!(cs.is_satisfied());
            assert!(cs.get("out bit/boolean") == Fr::from(0u64));
            assert!(!is_equal.get_value().unwrap());
            cs.set("out bit/boolean", Fr::from(1u64));
            assert!(!cs.is_satisfied());
        }

        {
            let mut cs = TestConstraintSystem::<Fr>::new();
            let a = AllocatedNum::alloc(cs.namespace(|| "a"), || Ok(Fr::random(&mut rng))).unwrap();
            let b = a.clone();
            let is_equal = a.is_equal(&mut cs, &b).unwrap();

            assert!(cs.is_satisfied());
            assert!(cs.get("out bit/boolean") == Fr::from(1u64));
            assert!(is_equal.get_value().unwrap());
            cs.set("out bit/boolean", Fr::from(0u64));
            assert!(!cs.is_satisfied());
        }
    }

    #[test]
    fn test_num_conditional_select() {
        let mut rng = XorShiftRng::from_seed([
            0x59, 0x62, 0xbe, 0x3d, 0x76, 0x3d, 0x31, 0x8d, 0x17, 0xdb, 0x37, 0x32, 0x54, 0x06,
            0xbc, 0xe5,
        ]);
        {
            let mut cs = TestConstraintSystem::<Fr>::new();

            let a = AllocatedNum::alloc(cs.namespace(|| "a"), || Ok(Fr::random(&mut rng))).unwrap();
            let b = AllocatedNum::alloc(cs.namespace(|| "b"), || Ok(Fr::random(&mut rng))).unwrap();
            let condition = Boolean::constant(false);
            let c = AllocatedNum::conditionally_select(&mut cs, &a, &b, &condition).unwrap();

            assert!(cs.is_satisfied());
            assert_eq!(a.value.unwrap(), c.value.unwrap());
        }

        {
            let mut cs = TestConstraintSystem::<Fr>::new();

            let a = AllocatedNum::alloc(cs.namespace(|| "a"), || Ok(Fr::random(&mut rng))).unwrap();
            let b = AllocatedNum::alloc(cs.namespace(|| "b"), || Ok(Fr::random(&mut rng))).unwrap();
            let condition = Boolean::constant(true);
            let c = AllocatedNum::conditionally_select(&mut cs, &a, &b, &condition).unwrap();

            assert!(cs.is_satisfied());
            assert_eq!(b.value.unwrap(), c.value.unwrap());
        }
    }

    #[test]
    fn test_num_conditional_reversal() {
        let mut rng = XorShiftRng::from_seed([
            0x59, 0x62, 0xbe, 0x3d, 0x76, 0x3d, 0x31, 0x8d, 0x17, 0xdb, 0x37, 0x32, 0x54, 0x06,
            0xbc, 0xe5,
        ]);
        {
            let mut cs = TestConstraintSystem::<Fr>::new();

            let a = AllocatedNum::alloc(cs.namespace(|| "a"), || Ok(Fr::random(&mut rng))).unwrap();
            let b = AllocatedNum::alloc(cs.namespace(|| "b"), || Ok(Fr::random(&mut rng))).unwrap();
            let condition = Boolean::constant(false);
            let (c, d) = AllocatedNum::conditionally_reverse(&mut cs, &a, &b, &condition).unwrap();

            assert!(cs.is_satisfied());

            assert_eq!(a.value.unwrap(), c.value.unwrap());
            assert_eq!(b.value.unwrap(), d.value.unwrap());
        }

        {
            let mut cs = TestConstraintSystem::<Fr>::new();

            let a = AllocatedNum::alloc(cs.namespace(|| "a"), || Ok(Fr::random(&mut rng))).unwrap();
            let b = AllocatedNum::alloc(cs.namespace(|| "b"), || Ok(Fr::random(&mut rng))).unwrap();
            let condition = Boolean::constant(true);
            let (c, d) = AllocatedNum::conditionally_reverse(&mut cs, &a, &b, &condition).unwrap();

            assert!(cs.is_satisfied());

            assert_eq!(a.value.unwrap(), d.value.unwrap());
            assert_eq!(b.value.unwrap(), c.value.unwrap());
        }
    }

    #[test]
    fn test_mux_tree() {
        let mut rng = XorShiftRng::from_seed([
            0x59, 0x62, 0xbe, 0x3d, 0x76, 0x3d, 0x31, 0x8d, 0x17, 0xdb, 0x37, 0x32, 0x54, 0x06,
            0xbc, 0xe5,
        ]);

        let conditions = vec![(false, false), (false, true), (true, false), (true, true)];

        for (c1, c0) in conditions {
            let mut cs = TestConstraintSystem::<Fr>::new();

            let condition0 = Boolean::constant(c0);
            let condition1 = Boolean::constant(c1);
            let select = &[condition1, condition0];

            let a0 = AllocatedNum::alloc(cs.namespace(|| "alloc a0"), || Ok(Fr::random(&mut rng)))
                .unwrap();
            let a1 = AllocatedNum::alloc(cs.namespace(|| "alloc a1"), || Ok(Fr::random(&mut rng)))
                .unwrap();
            let a2 = AllocatedNum::alloc(cs.namespace(|| "alloc a2"), || Ok(Fr::random(&mut rng)))
                .unwrap();
            let a3 = AllocatedNum::alloc(cs.namespace(|| "alloc a3"), || Ok(Fr::random(&mut rng)))
                .unwrap();

            let res = AllocatedNum::<Fr>::mux_tree(
                &mut cs.namespace(|| format!("mux tree result for conditions = {c1}, {c0}")),
                select.iter(),
                &[a0.clone(), a1.clone(), a2.clone(), a3.clone()],
            );
            assert!(res.is_ok());
            let res = res.unwrap();

            let res_expected = match (c1, c0) {
                (false, false) => a0.clone(),
                (false, true) => a1.clone(),
                (true, false) => a2.clone(),
                (true, true) => a3.clone(),
            };
            cs.enforce(
                || format!("res equality for conditions = {c1}, {c0}"),
                |lc| lc,
                |lc| lc,
                |lc| lc + res_expected.get_variable() - res.get_variable(),
            );

            assert!(cs.is_satisfied());
            assert_eq!(cs.num_constraints(), 4);
        }
    }

    #[test]
    fn test_num_nonzero() {
        {
            let mut cs = TestConstraintSystem::<Fr>::new();

            let n = AllocatedNum::alloc(&mut cs, || Ok(Fr::from(3u64))).unwrap();
            n.assert_nonzero(&mut cs).unwrap();

            assert!(cs.is_satisfied());
            cs.set("ephemeral inverse", Fr::from(3u64));
            assert!(cs.which_is_unsatisfied() == Some("nonzero assertion constraint"));
        }
        {
            let mut cs = TestConstraintSystem::<Fr>::new();

            let n = AllocatedNum::alloc(&mut cs, || Ok(Fr::ZERO)).unwrap();
            assert!(n.assert_nonzero(&mut cs).is_err());
        }
    }

    #[test]
    fn test_into_bits_strict() {
        let negone = -Fr::ONE;

        let mut cs = TestConstraintSystem::<Fr>::new();

        let n = AllocatedNum::alloc(&mut cs, || Ok(negone)).unwrap();
        n.to_bits_le_strict(&mut cs).unwrap();

        assert!(cs.is_satisfied());

        // make the bit representation the characteristic
        cs.set("bit 254/boolean", Fr::ONE);

        // this makes the conditional boolean constraint fail
        assert_eq!(
            cs.which_is_unsatisfied().unwrap(),
            "bit 254/boolean constraint"
        );
    }

    #[test]
    fn test_into_bits() {
        let mut rng = XorShiftRng::from_seed([
            0x59, 0x62, 0xbe, 0x3d, 0x76, 0x3d, 0x31, 0x8d, 0x17, 0xdb, 0x37, 0x32, 0x54, 0x06,
            0xbc, 0xe5,
        ]);

        for i in 0..200 {
            let r = Fr::random(&mut rng);
            let mut cs = TestConstraintSystem::<Fr>::new();

            let n = AllocatedNum::alloc(&mut cs, || Ok(r)).unwrap();

            let bits = if i % 2 == 0 {
                n.to_bits_le(&mut cs).unwrap()
            } else {
                n.to_bits_le_strict(&mut cs).unwrap()
            };

            assert!(cs.is_satisfied());

            for (i, b) in r.to_le_bits().iter().enumerate() {
                // `r.to_le_bits()` contains every bit in a representation (including bits which
                // exceed the field size), whereas the length of `bits` does not exceed the field
                // size.
                match bits.get(i) {
                    Some(Boolean::Is(a)) => assert_eq!(b, a.get_value().unwrap()),
                    Some(_) => unreachable!(),
                    None => assert!(!b),
                };
            }

            cs.set("num", Fr::random(&mut rng));
            assert!(!cs.is_satisfied());
            cs.set("num", r);
            assert!(cs.is_satisfied());

            for i in 0..Fr::NUM_BITS {
                let name = format!("bit {}/boolean", i);
                let cur = cs.get(&name);
                let mut tmp = Fr::ONE;
                tmp.sub_assign(&cur);
                cs.set(&name, tmp);
                assert!(!cs.is_satisfied());
                cs.set(&name, cur);
                assert!(cs.is_satisfied());
            }
        }
    }

    #[test]
    fn test_num_scale() {
        use crate::{Index, LinearCombination, Variable};

        let mut rng = XorShiftRng::from_seed([
            0x59, 0x62, 0xbe, 0x3d, 0x76, 0x3d, 0x31, 0x8d, 0x17, 0xdb, 0x37, 0x32, 0x54, 0x06,
            0xbc, 0xe5,
        ]);

        let n = 5;

        let mut lc = LinearCombination::<Fr>::zero();

        let mut expected_sums = vec![Fr::ZERO; n];
        let mut value = Fr::ZERO;
        for (i, expected_sum) in expected_sums.iter_mut().enumerate() {
            let coeff = Fr::random(&mut rng);
            lc = lc + (coeff, Variable::new_unchecked(Index::Aux(i)));
            expected_sum.add_assign(&coeff);

            value.add_assign(&coeff);
        }

        let scalar = Fr::random(&mut rng);
        let num = Num {
            value: Some(value),
            lc,
        };

        let scaled_num = num.clone().scale(scalar);

        let mut scaled_value = num.value.unwrap();
        scaled_value.mul_assign(&scalar);

        assert_eq!(scaled_value, scaled_num.value.unwrap());

        // Each variable has the expected coefficient, the sume of those added by its Index.
        scaled_num.lc.iter().for_each(|(var, coeff)| match var.0 {
            Index::Aux(i) => {
                let mut tmp = expected_sums[i];
                tmp.mul_assign(&scalar);
                assert_eq!(tmp, *coeff)
            }
            _ => panic!("unexpected variable type"),
        });
    }
}

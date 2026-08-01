//! Shared evaluation of the builtin functions.
//!
//! Both execution engines (the stack [`vm`](crate::backend::vm) and the
//! [`mir::interp`](crate::mir::interp) interpreter) call into here, so a builtin
//! behaves identically no matter which engine runs the program. Output from the
//! `print_*` family is appended to the caller's buffer.

use std::cell::RefCell;
use std::rc::Rc;

use crate::backend::bytecode::Value;
use crate::backend::vm::{MAX_ARRAY_LEN, VmError, index_in_bounds};
use crate::sema::types::{Builtin, Elem};

/// Evaluates a builtin over already-evaluated arguments, appending any printed
/// output to `out`. The type checker guarantees argument arity and types, so a
/// shape mismatch here is an internal error rather than a user error.
pub fn eval(builtin: Builtin, args: &[Value], out: &mut String) -> Result<Value, VmError> {
    let bad = || VmError::Internal("builtin called with wrong argument");
    let print = |out: &mut String, text: String| {
        out.push_str(&text);
        out.push('\n');
        Value::Unit
    };
    Ok(match (builtin, args) {
        // ---- print family (I/O, returns unit) ----
        (Builtin::PrintInt, [Value::Int(v)]) => print(out, v.to_string()),
        (Builtin::PrintFloat, [Value::Float(v)]) => print(out, v.to_string()),
        (Builtin::PrintBool, [Value::Bool(v)]) => print(out, v.to_string()),
        (Builtin::PrintStr, [Value::Str(v)]) => print(out, v.to_string()),
        // ---- conversions ----
        (Builtin::IntToStr, [Value::Int(v)]) => Value::Str(v.to_string().into()),
        (Builtin::FloatToStr, [Value::Float(v)]) => Value::Str(v.to_string().into()),
        (Builtin::BoolToStr, [Value::Bool(v)]) => Value::Str(v.to_string().into()),
        (Builtin::StrLen, [Value::Str(v)]) => Value::Int(v.len() as i64),
        // ---- integer math ----
        (Builtin::AbsInt, [Value::Int(v)]) => Value::Int(v.wrapping_abs()),
        (Builtin::MinInt, [Value::Int(a), Value::Int(b)]) => Value::Int((*a).min(*b)),
        (Builtin::MaxInt, [Value::Int(a), Value::Int(b)]) => Value::Int((*a).max(*b)),
        (Builtin::PowInt, [Value::Int(a), Value::Int(b)]) => {
            let exp = u32::try_from(*b).map_err(|_| VmError::Internal("negative exponent"))?;
            Value::Int(a.wrapping_pow(exp))
        }
        (Builtin::Gcd, [Value::Int(a), Value::Int(b)]) => Value::Int(gcd(*a, *b)),
        (Builtin::Lcm, [Value::Int(a), Value::Int(b)]) => {
            let g = gcd(*a, *b);
            // lcm(0, x) = 0; otherwise |a / gcd * b|.
            Value::Int(if g == 0 {
                0
            } else {
                (a / g).wrapping_mul(*b).wrapping_abs()
            })
        }
        (Builtin::SignInt, [Value::Int(v)]) => Value::Int(v.signum()),
        (Builtin::ClampInt, [Value::Int(v), Value::Int(lo), Value::Int(hi)]) => {
            // A reversed `[lo, hi]` would panic in `clamp`; treat it as `lo`.
            Value::Int(if lo > hi { *lo } else { (*v).clamp(*lo, *hi) })
        }
        // ---- float math ----
        (Builtin::Sqrt, [Value::Float(v)]) => Value::Float(v.sqrt()),
        (Builtin::AbsFloat, [Value::Float(v)]) => Value::Float(v.abs()),
        (Builtin::Floor, [Value::Float(v)]) => Value::Int(v.floor() as i64),
        (Builtin::PowFloat, [Value::Float(a), Value::Float(b)]) => Value::Float(a.powf(*b)),
        (Builtin::MinFloat, [Value::Float(a), Value::Float(b)]) => Value::Float(a.min(*b)),
        (Builtin::MaxFloat, [Value::Float(a), Value::Float(b)]) => Value::Float(a.max(*b)),
        (Builtin::Ceil, [Value::Float(v)]) => Value::Int(v.ceil() as i64),
        (Builtin::Round, [Value::Float(v)]) => Value::Int(v.round() as i64),
        // ---- numeric conversions ----
        (Builtin::IntToFloat, [Value::Int(v)]) => Value::Float(*v as f64),
        (Builtin::FloatToInt, [Value::Float(v)]) => Value::Int(*v as i64),
        // ---- string ----
        (Builtin::Substring, [Value::Str(s), Value::Int(start), Value::Int(end)]) => {
            Value::Str(substring(s, *start, *end).into())
        }
        (Builtin::CharAt, [Value::Str(s), Value::Int(i)]) => {
            let bytes = s.as_bytes();
            match index_in_bounds(*i, bytes.len()) {
                Some(idx) => Value::Int(bytes[idx] as i64),
                None => {
                    return Err(VmError::IndexOutOfBounds {
                        index: *i,
                        len: bytes.len(),
                    });
                }
            }
        }
        (Builtin::StrRepeat, [Value::Str(s), Value::Int(n)]) => {
            let count = (*n).max(0) as usize;
            Value::Str(s.repeat(count).into())
        }
        (Builtin::StartsWith, [Value::Str(s), Value::Str(p)]) => Value::Bool(s.starts_with(&**p)),
        (Builtin::EndsWith, [Value::Str(s), Value::Str(p)]) => Value::Bool(s.ends_with(&**p)),
        (Builtin::Contains, [Value::Str(s), Value::Str(p)]) => Value::Bool(s.contains(&**p)),
        (Builtin::IndexOf, [Value::Str(s), Value::Str(p)]) => {
            Value::Int(s.find(&**p).map(|i| i as i64).unwrap_or(-1))
        }
        (Builtin::ParseInt, [Value::Str(s)]) => Value::Int(s.trim().parse::<i64>().unwrap_or(0)),
        (Builtin::ToUpper, [Value::Str(s)]) => Value::Str(s.to_ascii_uppercase().into()),
        (Builtin::ToLower, [Value::Str(s)]) => Value::Str(s.to_ascii_lowercase().into()),
        (Builtin::Trim, [Value::Str(s)]) => Value::Str(s.trim().into()),
        (Builtin::CharToStr, [Value::Int(v)]) => {
            // A byte value in range yields its one-byte string; otherwise empty.
            let byte = u8::try_from(*v).ok();
            let s = byte
                .filter(|b| b.is_ascii())
                .map(|b| (b as char).to_string())
                .unwrap_or_default();
            Value::Str(s.into())
        }
        // ---- array construction ----
        (Builtin::ArrayNewInt, [Value::Int(n)]) => new_array(Elem::Int, *n)?,
        (Builtin::ArrayNewFloat, [Value::Int(n)]) => new_array(Elem::Float, *n)?,
        (Builtin::ArrayNewBool, [Value::Int(n)]) => new_array(Elem::Bool, *n)?,
        (Builtin::ArrayNewStr, [Value::Int(n)]) => new_array(Elem::Str, *n)?,
        _ => return Err(bad()),
    })
}

/// Allocates an array of `len` zero values of element type `elem`.
///
/// This is the one definition of what `array_new_*` means. The stack VM
/// reaches it through [`Op::NewArray`](crate::backend::bytecode::Op::NewArray)
/// and the MIR interpreter through [`eval`] above, so the two engines cannot
/// drift apart.
///
/// A negative length is a program error. An excessive one is refused rather
/// than handed to the allocator, so it fails as a [`VmError`] instead of
/// aborting the process.
pub fn new_array(elem: Elem, len: i64) -> Result<Value, VmError> {
    if len < 0 {
        return Err(VmError::NegativeArrayLength(len));
    }
    if len > MAX_ARRAY_LEN {
        return Err(VmError::ArrayTooLong {
            len,
            max: MAX_ARRAY_LEN,
        });
    }
    let items = vec![zero_value(elem); len as usize];
    Ok(Value::Array(Rc::new(RefCell::new(items))))
}

/// The zero value an `array_new_*` fills its array with.
fn zero_value(elem: Elem) -> Value {
    match elem {
        Elem::Int => Value::Int(0),
        Elem::Float => Value::Float(0.0),
        Elem::Bool => Value::Bool(false),
        Elem::Str => Value::Str("".into()),
    }
}

/// Greatest common divisor of two integers' magnitudes (Euclid's algorithm).
/// `gcd(0, 0)` is `0`.
fn gcd(a: i64, b: i64) -> i64 {
    let mut a = a.unsigned_abs();
    let mut b = b.unsigned_abs();
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a as i64
}

/// Returns the byte substring `[start, end)` of `s`, clamped to bounds. If the
/// range does not fall on character boundaries, yields the empty string rather
/// than panicking.
fn substring(s: &str, start: i64, end: i64) -> String {
    let len = s.len() as i64;
    let start = start.clamp(0, len) as usize;
    let end = end.clamp(0, len) as usize;
    if start >= end {
        return String::new();
    }
    s.get(start..end).unwrap_or("").to_string()
}

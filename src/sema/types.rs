//! The semantic type representation, shared by type checking, HIR, and codegen.
//!
//! Lumen is monomorphic with a closed set of primitive types, so [`Type`] is a
//! small `Copy` enum rather than an interned/structured type. The [`Type::Error`]
//! variant is a *poison* value: it is produced wherever a type error has already
//! been reported and then absorbs further checks involving it, which prevents a
//! single mistake from cascading into a flood of follow-on diagnostics.

/// The element type of an array. A `Copy` sub-enum of the primitive types,
/// which keeps [`Type`] itself `Copy` while still supporting `[i64]`, `[str]`,
/// etc. Nested arrays are intentionally out of scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Elem {
    Int,
    Float,
    Bool,
    Str,
}

impl Elem {
    /// The full [`Type`] of this element.
    pub fn ty(self) -> Type {
        match self {
            Elem::Int => Type::Int,
            Elem::Float => Type::Float,
            Elem::Bool => Type::Bool,
            Elem::Str => Type::Str,
        }
    }

    /// The element kind for a primitive type, if it can be an array element.
    pub fn of(ty: Type) -> Option<Elem> {
        Some(match ty {
            Type::Int => Elem::Int,
            Type::Float => Elem::Float,
            Type::Bool => Elem::Bool,
            Type::Str => Elem::Str,
            _ => return None,
        })
    }
}

/// A Lumen value type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Type {
    /// 64-bit signed integer.
    Int,
    /// 64-bit IEEE-754 float.
    Float,
    /// Boolean.
    Bool,
    /// Immutable UTF-8 string.
    Str,
    /// The empty type, value of statements and `()`-returning functions.
    Unit,
    /// A growable array of a primitive element type.
    Array(Elem),
    /// A user-defined struct, identified by its dense struct index. The raw
    /// `u32` (rather than a `resolve::StructId`) keeps this module free of any
    /// dependency on name resolution.
    Struct(u32),
    /// A tuple, identified by its interned (structural) tuple-type index. The
    /// element types live in a side table owned by the type checker.
    Tuple(u32),
    /// The error type  already-reported, absorbs further checks.
    Error,
}

impl Type {
    /// Maps a primitive type *name* (as written in source) to its [`Type`].
    /// Returns `None` for unknown names, which the caller reports.
    pub fn from_name(name: &str) -> Option<Type> {
        Some(match name {
            "i64" => Type::Int,
            "f64" => Type::Float,
            "bool" => Type::Bool,
            "str" => Type::Str,
            "unit" => Type::Unit,
            _ => return None,
        })
    }

    /// The array type with the given element type.
    pub fn array_of(elem: Elem) -> Type {
        Type::Array(elem)
    }

    /// Whether this is the poison [`Type::Error`].
    pub fn is_error(self) -> bool {
        matches!(self, Type::Error)
    }

    /// Whether this is an array type.
    pub fn is_array(self) -> bool {
        matches!(self, Type::Array(_))
    }

    /// The element type if this is an array.
    pub fn elem(self) -> Option<Elem> {
        match self {
            Type::Array(e) => Some(e),
            _ => None,
        }
    }

    /// Whether two types are compatible for assignment/unification.
    ///
    /// [`Type::Error`] is compatible with everything, so that an
    /// already-reported error never triggers a second, spurious mismatch.
    pub fn compatible(self, other: Type) -> bool {
        self.is_error() || other.is_error() || self == other
    }

    /// Whether arithmetic operators (`+ - * / %`) apply to this type.
    pub fn is_numeric(self) -> bool {
        matches!(self, Type::Int | Type::Float)
    }
}

impl std::fmt::Display for Type {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Type::Int => f.write_str("i64"),
            Type::Float => f.write_str("f64"),
            Type::Bool => f.write_str("bool"),
            Type::Str => f.write_str("str"),
            Type::Unit => f.write_str("unit"),
            Type::Error => f.write_str("{error}"),
            Type::Array(elem) => write!(f, "[{}]", elem.ty()),
            Type::Struct(id) => write!(f, "struct#{id}"),
            Type::Tuple(id) => write!(f, "tuple#{id}"),
        }
    }
}

/// A compiler-provided function available to every program without declaration.
///
/// Builtins keep the language's surface tiny while still allowing real I/O in
/// examples and tests. Each has a fixed monomorphic signature; the VM supplies
/// the implementation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Builtin {
    /// `print_int(i64) -> unit`
    PrintInt,
    /// `print_float(f64) -> unit`
    PrintFloat,
    /// `print_bool(bool) -> unit`
    PrintBool,
    /// `print_str(str) -> unit`
    PrintStr,
    /// `int_to_str(i64) -> str`
    IntToStr,
    /// `float_to_str(f64) -> str`
    FloatToStr,
    /// `bool_to_str(bool) -> str`
    BoolToStr,
    /// `str_len(str) -> i64` - length in bytes.
    StrLen,
    /// `len([T]) -> i64` - array length. Generic over the element type, so it
    /// is checked specially rather than through [`Builtin::params`].
    Len,

    // ---- integer math ----
    /// `abs(i64) -> i64`
    AbsInt,
    /// `min(i64, i64) -> i64`
    MinInt,
    /// `max(i64, i64) -> i64`
    MaxInt,
    /// `pow_int(i64, i64) -> i64` - non-negative exponent.
    PowInt,
    /// `gcd(i64, i64) -> i64` - greatest common divisor of the magnitudes.
    Gcd,
    /// `sign(i64) -> i64` - `-1`, `0`, or `1`.
    SignInt,
    /// `clamp(i64, i64, i64) -> i64` - clamp a value to `[lo, hi]`.
    ClampInt,

    // ---- float math ----
    /// `sqrt(f64) -> f64`
    Sqrt,
    /// `abs_float(f64) -> f64`
    AbsFloat,
    /// `floor(f64) -> i64`
    Floor,
    /// `pow_float(f64, f64) -> f64`
    PowFloat,
    /// `min_float(f64, f64) -> f64`
    MinFloat,
    /// `max_float(f64, f64) -> f64`
    MaxFloat,
    /// `ceil(f64) -> i64`
    Ceil,
    /// `round(f64) -> i64` - round half away from zero.
    Round,

    // ---- numeric conversions ----
    /// `to_float(i64) -> f64`
    IntToFloat,
    /// `to_int(f64) -> i64` - truncates toward zero.
    FloatToInt,

    // ---- string ----
    /// `substring(str, i64, i64) -> str` - byte range `[start, end)`.
    Substring,
    /// `char_at(str, i64) -> i64` - the byte at an index.
    CharAt,
    /// `str_repeat(str, i64) -> str`
    StrRepeat,
    /// `starts_with(str, str) -> bool`
    StartsWith,
    /// `ends_with(str, str) -> bool`
    EndsWith,
    /// `contains(str, str) -> bool`
    Contains,
    /// `index_of(str, str) -> i64` - byte offset of the first match, or `-1`.
    IndexOf,
    /// `parse_int(str) -> i64` - parse a decimal integer, or `0` on failure.
    ParseInt,
    /// `char_to_str(i64) -> str` - the one-byte string for a byte value.
    CharToStr,
    /// `to_upper(str) -> str` - ASCII upper-casing.
    ToUpper,
    /// `to_lower(str) -> str` - ASCII lower-casing.
    ToLower,
    /// `trim(str) -> str` - strip leading and trailing ASCII whitespace.
    Trim,

    // ---- more integer math ----
    /// `lcm(i64, i64) -> i64` - least common multiple (`0` if either is `0`).
    Lcm,

    // ---- array construction ----
    // Lumen has no generics, so allocating an array is one builtin per element
    // type rather than a single polymorphic `array_new`. Each yields an array
    // of `n` zero values, where `n` may be a runtime value.
    /// `array_new_int(i64) -> [i64]`
    ArrayNewInt,
    /// `array_new_float(i64) -> [f64]`
    ArrayNewFloat,
    /// `array_new_bool(i64) -> [bool]`
    ArrayNewBool,
    /// `array_new_str(i64) -> [str]`
    ArrayNewStr,
}

impl Builtin {
    /// Every builtin, used to seed the global name scope.
    pub const ALL: [Builtin; 43] = [
        Builtin::PrintInt,
        Builtin::PrintFloat,
        Builtin::PrintBool,
        Builtin::PrintStr,
        Builtin::IntToStr,
        Builtin::FloatToStr,
        Builtin::BoolToStr,
        Builtin::StrLen,
        Builtin::Len,
        Builtin::AbsInt,
        Builtin::MinInt,
        Builtin::MaxInt,
        Builtin::PowInt,
        Builtin::Gcd,
        Builtin::SignInt,
        Builtin::ClampInt,
        Builtin::Sqrt,
        Builtin::AbsFloat,
        Builtin::Floor,
        Builtin::PowFloat,
        Builtin::MinFloat,
        Builtin::MaxFloat,
        Builtin::Ceil,
        Builtin::Round,
        Builtin::IntToFloat,
        Builtin::FloatToInt,
        Builtin::Substring,
        Builtin::CharAt,
        Builtin::StrRepeat,
        Builtin::StartsWith,
        Builtin::EndsWith,
        Builtin::Contains,
        Builtin::IndexOf,
        Builtin::ParseInt,
        Builtin::CharToStr,
        Builtin::ToUpper,
        Builtin::ToLower,
        Builtin::Trim,
        Builtin::Lcm,
        Builtin::ArrayNewInt,
        Builtin::ArrayNewFloat,
        Builtin::ArrayNewBool,
        Builtin::ArrayNewStr,
    ];

    /// The name programs call this builtin by.
    pub fn name(self) -> &'static str {
        match self {
            Builtin::PrintInt => "print_int",
            Builtin::PrintFloat => "print_float",
            Builtin::PrintBool => "print_bool",
            Builtin::PrintStr => "print_str",
            Builtin::IntToStr => "int_to_str",
            Builtin::FloatToStr => "float_to_str",
            Builtin::BoolToStr => "bool_to_str",
            Builtin::StrLen => "str_len",
            Builtin::Len => "len",
            Builtin::AbsInt => "abs",
            Builtin::MinInt => "min",
            Builtin::MaxInt => "max",
            Builtin::PowInt => "pow_int",
            Builtin::Gcd => "gcd",
            Builtin::SignInt => "sign",
            Builtin::ClampInt => "clamp",
            Builtin::Sqrt => "sqrt",
            Builtin::AbsFloat => "abs_float",
            Builtin::Floor => "floor",
            Builtin::PowFloat => "pow_float",
            Builtin::MinFloat => "min_float",
            Builtin::MaxFloat => "max_float",
            Builtin::Ceil => "ceil",
            Builtin::Round => "round",
            Builtin::IntToFloat => "to_float",
            Builtin::FloatToInt => "to_int",
            Builtin::Substring => "substring",
            Builtin::CharAt => "char_at",
            Builtin::StrRepeat => "str_repeat",
            Builtin::StartsWith => "starts_with",
            Builtin::EndsWith => "ends_with",
            Builtin::Contains => "contains",
            Builtin::IndexOf => "index_of",
            Builtin::ParseInt => "parse_int",
            Builtin::CharToStr => "char_to_str",
            Builtin::ToUpper => "to_upper",
            Builtin::ToLower => "to_lower",
            Builtin::Trim => "trim",
            Builtin::Lcm => "lcm",
            Builtin::ArrayNewInt => "array_new_int",
            Builtin::ArrayNewFloat => "array_new_float",
            Builtin::ArrayNewBool => "array_new_bool",
            Builtin::ArrayNewStr => "array_new_str",
        }
    }

    /// The builtin invoked by `name`, if any.
    pub fn from_name(name: &str) -> Option<Builtin> {
        Builtin::ALL.into_iter().find(|b| b.name() == name)
    }

    /// Whether this builtin's signature is generic and therefore checked
    /// specially (it cannot be described by a fixed [`Builtin::params`] list).
    pub fn is_generic(self) -> bool {
        matches!(self, Builtin::Len)
    }

    /// The element type this builtin allocates, if it is one of the
    /// `array_new_*` family. This is the single mapping from builtin to element
    /// type, so the code generator does not repeat it.
    pub fn new_array_elem(self) -> Option<Elem> {
        Some(match self {
            Builtin::ArrayNewInt => Elem::Int,
            Builtin::ArrayNewFloat => Elem::Float,
            Builtin::ArrayNewBool => Elem::Bool,
            Builtin::ArrayNewStr => Elem::Str,
            _ => return None,
        })
    }

    /// The parameter types this builtin accepts. Empty for [generic] builtins.
    ///
    /// [generic]: Builtin::is_generic
    pub fn params(self) -> &'static [Type] {
        match self {
            Builtin::PrintInt | Builtin::IntToStr => &[Type::Int],
            Builtin::PrintFloat | Builtin::FloatToStr => &[Type::Float],
            Builtin::PrintBool | Builtin::BoolToStr => &[Type::Bool],
            Builtin::PrintStr | Builtin::StrLen => &[Type::Str],
            Builtin::Len => &[],
            Builtin::AbsInt | Builtin::SignInt => &[Type::Int],
            Builtin::MinInt | Builtin::MaxInt | Builtin::PowInt | Builtin::Gcd | Builtin::Lcm => {
                &[Type::Int, Type::Int]
            }
            Builtin::ClampInt => &[Type::Int, Type::Int, Type::Int],
            Builtin::Sqrt
            | Builtin::AbsFloat
            | Builtin::Floor
            | Builtin::Ceil
            | Builtin::Round
            | Builtin::FloatToInt => &[Type::Float],
            Builtin::IntToFloat => &[Type::Int],
            Builtin::PowFloat | Builtin::MinFloat | Builtin::MaxFloat => {
                &[Type::Float, Type::Float]
            }
            Builtin::Substring => &[Type::Str, Type::Int, Type::Int],
            Builtin::CharAt => &[Type::Str, Type::Int],
            Builtin::StrRepeat => &[Type::Str, Type::Int],
            Builtin::StartsWith | Builtin::EndsWith | Builtin::Contains | Builtin::IndexOf => {
                &[Type::Str, Type::Str]
            }
            Builtin::ParseInt | Builtin::ToUpper | Builtin::ToLower | Builtin::Trim => &[Type::Str],
            Builtin::CharToStr => &[Type::Int],
            Builtin::ArrayNewInt
            | Builtin::ArrayNewFloat
            | Builtin::ArrayNewBool
            | Builtin::ArrayNewStr => &[Type::Int],
        }
    }

    /// The result type of this builtin.
    pub fn ret(self) -> Type {
        match self {
            Builtin::PrintInt | Builtin::PrintFloat | Builtin::PrintBool | Builtin::PrintStr => {
                Type::Unit
            }
            Builtin::IntToStr
            | Builtin::FloatToStr
            | Builtin::BoolToStr
            | Builtin::Substring
            | Builtin::StrRepeat
            | Builtin::CharToStr
            | Builtin::ToUpper
            | Builtin::ToLower
            | Builtin::Trim => Type::Str,
            Builtin::StrLen
            | Builtin::Len
            | Builtin::Floor
            | Builtin::Ceil
            | Builtin::Round
            | Builtin::FloatToInt
            | Builtin::CharAt
            | Builtin::IndexOf
            | Builtin::ParseInt => Type::Int,
            Builtin::IntToFloat => Type::Float,
            Builtin::AbsInt
            | Builtin::MinInt
            | Builtin::MaxInt
            | Builtin::PowInt
            | Builtin::Gcd
            | Builtin::Lcm
            | Builtin::SignInt
            | Builtin::ClampInt => Type::Int,
            Builtin::Sqrt
            | Builtin::AbsFloat
            | Builtin::PowFloat
            | Builtin::MinFloat
            | Builtin::MaxFloat => Type::Float,
            Builtin::StartsWith | Builtin::EndsWith | Builtin::Contains => Type::Bool,
            Builtin::ArrayNewInt => Type::Array(Elem::Int),
            Builtin::ArrayNewFloat => Type::Array(Elem::Float),
            Builtin::ArrayNewBool => Type::Array(Elem::Bool),
            Builtin::ArrayNewStr => Type::Array(Elem::Str),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primitive_name_round_trip() {
        for ty in [Type::Int, Type::Float, Type::Bool, Type::Str, Type::Unit] {
            assert_eq!(Type::from_name(&ty.to_string()), Some(ty));
        }
        assert_eq!(Type::from_name("nope"), None);
    }

    #[test]
    fn error_is_compatible_with_anything() {
        assert!(Type::Error.compatible(Type::Int));
        assert!(Type::Int.compatible(Type::Error));
        assert!(!Type::Int.compatible(Type::Bool));
    }

    #[test]
    fn builtin_lookup() {
        assert_eq!(Builtin::from_name("print_int"), Some(Builtin::PrintInt));
        assert_eq!(
            Builtin::from_name("print_int").unwrap().params(),
            &[Type::Int]
        );
        assert_eq!(Builtin::from_name("nope"), None);
    }
}

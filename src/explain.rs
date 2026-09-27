//! Long-form explanations of diagnostic codes, shown by `lumenc explain`.
//!
//! Each explanation says why the error happens and gives a minimal example.
//! The match in [`explain_code`] is exhaustive, so a new [`DiagCode`] cannot
//! ship without one.

use crate::errors::DiagCode;

/// The explanation for a code string such as `"E0300"`, or `None` if the code
/// is unknown.
pub fn explain(code: &str) -> Option<&'static str> {
    DiagCode::ALL
        .iter()
        .copied()
        .find(|c| c.as_str() == code)
        .map(explain_code)
}

/// The explanation for `code`.
pub fn explain_code(code: DiagCode) -> &'static str {
    use DiagCode::*;
    match code {
        UnexpectedChar => {
            "\
E0001: an unexpected character.

The lexer found a character that cannot begin any token. This is usually a typo
or a stray symbol that is not part of Lumen's syntax.

    fn main() { let x = 1 @ 2; }   // `@` is not a Lumen operator
"
        }
        UnterminatedString => {
            "\
E0002: an unterminated string literal.

A string literal was opened with `\"` but never closed before the end of the
line or file. Add the missing closing quote.

    let s = \"hello;     // error: no closing quote
    let s = \"hello\";    // ok
"
        }
        InvalidNumber => {
            "\
E0003: an invalid numeric literal.

A run of digits did not parse as a 64-bit number, usually because it is too
large to fit in a signed 64-bit integer.

    let big = 99999999999999999999999;   // does not fit in i64
"
        }
        UnterminatedComment => {
            "\
E0004: an unterminated block comment.

A `/* */` comment (which may nest) was opened but never closed. Add the matching
`*/`.

    /* a comment that never ends
"
        }
        InvalidEscape => {
            "\
E0005: an unknown string escape.

A backslash inside a string was followed by a character that is not a valid
escape. The valid escapes are \\n \\t \\r \\0 \\\\ and \\\".

    let s = \"bad\\q\";    // \\q is not an escape
"
        }
        UnexpectedToken => {
            "\
E0100: an unexpected token.

The parser found a token where the grammar did not allow it. The message names
what was expected and what was found.

    fn main() { let x 1; }   // expected `=`, found integer literal
"
        }
        UnexpectedEof => {
            "\
E0101: unexpected end of file.

Input ended in the middle of a construct, such as an unclosed block or a
statement missing its end.

    fn main() {            // the block is never closed
"
        }
        ExpectedType => {
            "\
E0102: expected a type.

A type annotation was required (after `:` or `->`) but something other than a
type was found.

    fn f(x: ) {}   // missing the parameter type
"
        }
        UnresolvedName => {
            "\
E0200: cannot find a name in scope.

A name was used that is not a local variable, parameter, function, constant, or
builtin visible here. Check for typos, or declare the binding first.

    fn main() { print_int(x); }   // `x` was never declared
"
        }
        DuplicateDefinition => {
            "\
E0201: a name is defined multiple times.

Two top-level items (functions, constants, or structs) share a name. Top-level
names occupy one namespace and must be unique.

    fn f() {}
    fn f() {}    // error: `f` already defined
"
        }
        DuplicateParameter => {
            "\
E0202: a parameter name is repeated.

A function declares the same parameter name twice. Parameter names within one
function must be distinct.

    fn f(x: i64, x: i64) {}   // `x` declared twice
"
        }
        TypeMismatch => {
            "\
E0300: mismatched types.

A value's type did not match the type required by its context - an annotation, a
function parameter, an assignment, or an operator. Lumen performs no implicit
conversions.

    let x: i64 = \"hello\";   // expected i64, found str
"
        }
        ArityMismatch => {
            "\
E0301: wrong number of arguments.

A call passed a different number of arguments than the function or builtin
expects.

    fn f(a: i64) {}
    fn main() { f(1, 2); }   // f takes 1 argument, 2 given
"
        }
        NotCallable => {
            "\
E0302: value is not callable.

Something that is not a function was called with `()`. Only functions and
builtins can be called.

    fn main() { let x = 1; x(); }   // `x` is an i64, not a function
"
        }
        ReturnTypeMismatch => {
            "\
E0303: return type mismatch.

A `return` (or a function's final expression) produced a value whose type does
not match the declared return type.

    fn f() -> i64 { return true; }   // expected i64, found bool
"
        }
        NonBoolCondition => {
            "\
E0304: a condition is not a boolean.

The condition of an `if` or `while` must have type `bool`. Lumen does not treat
integers as truthy.

    fn main() { if 1 { } }   // expected bool, found i64
"
        }
        InvalidOperands => {
            "\
E0305: invalid operands for an operator.

An operator was applied to operands of the wrong type - for example arithmetic
on booleans, mixing `i64` and `f64`, or `&&` on non-booleans.

    let x = 1 + 1.0;   // cannot add i64 and f64
"
        }
        MissingReturn => {
            "\
E0306: a function may not return a value.

A function with a non-`unit` return type can reach its end without returning.
Every path must produce a value.

    fn f(b: bool) -> i64 { if b { return 1; } }   // falls off the end
"
        }
        AssignToImmutable => {
            "\
E0307: cannot assign to an immutable binding.

Assignment targets a `let` binding that was not declared `mut`. Declare it with
`let mut` to allow reassignment.

    let x = 1; x = 2;        // error
    let mut x = 1; x = 2;    // ok
"
        }
        BadMain => {
            "\
E0308: invalid or missing `main`.

A program must define exactly one entry point `fn main()` taking no arguments
and returning `unit`.

    fn main(x: i64) {}   // main must take no arguments
"
        }
        IfBranchMismatch => {
            "\
E0309: `if` branches have incompatible types.

When an `if` is used as a value, its `then` and `else` branches must have the
same type; an `if` without `else` has type `unit`.

    let x = if c { 1 } else { false };   // i64 vs bool
"
        }
        UnknownType => {
            "\
E0310: unknown type.

A type annotation named a type that does not exist. The built-in types are i64,
f64, bool, str, and unit; struct types must be declared.

    fn f(x: i32) {}   // did you mean i64?
"
        }
        InvalidAssignTarget => {
            "\
E0311: invalid assignment target.

The left-hand side of an assignment is not a place that can be assigned to (a
variable, array element, or struct field).

    fn main() { 1 + 1 = 2; }   // cannot assign to an expression
"
        }
        BreakOutsideLoop => {
            "\
E0312: `break`/`continue` outside a loop.

`break` and `continue` are only meaningful inside a `while` or `for` loop.

    fn main() { break; }   // not in a loop
"
        }
        NotConstant => {
            "\
E0313: a `const` value is not constant.

A `const` initialiser must be evaluable at compile time: literals, operators, and
references to earlier constants - never function calls or variables.

    fn f() -> i64 { 1 }
    const N: i64 = f();   // calls are not constant
"
        }
        BadArrayType => {
            "\
E0314: invalid array type or empty literal.

Array elements must be a primitive type (i64, f64, bool, str), and an empty
array literal `[]` cannot have its type inferred.

    let a = [];   // cannot infer element type
"
        }
        NotIndexable => {
            "\
E0315: value cannot be indexed.

The `[]` index operator was applied to something that is not an array.

    fn main() { let x = 1; print_int(x[0]); }   // i64 is not indexable
"
        }
        UnknownField => {
            "\
E0316: no such field.

A field was accessed that the struct does not declare, or `.field` was used on a
non-struct value.

    struct P { x: i64 }
    fn main() { let p = P { x: 1 }; print_int(p.y); }   // no field `y`
"
        }
        BadStructLiteral => {
            "\
E0317: malformed struct literal.

A struct literal is missing fields, repeats a field, names a field that does not
exist, or names a type that is not a struct. Every declared field must be given
exactly once.

    struct P { x: i64, y: i64 }
    let p = P { x: 1 };   // missing field `y`
"
        }
        NonExhaustiveMatch => {
            "\
E0318: non-exhaustive `match`.

A `match` expression must handle every possible value of its scrutinee. Add the
missing patterns, or a wildcard `_` arm that catches everything else.

    match n {
        0 => print_str(\"zero\"),
        _ => print_str(\"other\"),   // without this arm, `match` is incomplete
    }
"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::DiagCode;

    #[test]
    fn every_code_has_an_explanation() {
        for code in DiagCode::ALL {
            let text = explain_code(code);
            assert!(
                text.starts_with(code.as_str()),
                "explanation for {} mislabelled",
                code.as_str()
            );
            assert!(
                text.len() > 40,
                "explanation for {} too short",
                code.as_str()
            );
        }
    }

    #[test]
    fn explain_by_string() {
        assert!(explain("E0300").unwrap().contains("mismatched types"));
        assert_eq!(explain("E9999"), None);
    }
}

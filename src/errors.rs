//! The registry of diagnostic codes.
//!
//! Every user-facing error carries a [`DiagCode`], grouped by phase:
//!
//! | Codes   | Phase           |
//! |---------|-----------------|
//! | `E00xx` | lexer           |
//! | `E01xx` | parser          |
//! | `E02xx` | name resolution |
//! | `E03xx` | type checking   |
//!
//! Codes are append-only: once shipped, a code keeps its meaning.

/// A stable, user-visible diagnostic code such as `E0301`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DiagCode {
    // ---- Lexer (E00xx) ----
    /// A character that cannot begin any token.
    UnexpectedChar,
    /// A string literal without a closing quote.
    UnterminatedString,
    /// An integer literal too large for `i64`.
    InvalidNumber,
    /// A `/* */` comment without a closing `*/`.
    UnterminatedComment,
    /// An unknown string escape such as `\q`.
    InvalidEscape,

    // ---- Parser (E01xx) ----
    /// A token appeared where the grammar did not allow it.
    UnexpectedToken,
    /// Input ended in the middle of a construct.
    UnexpectedEof,
    /// A type name was expected but something else was found.
    ExpectedType,

    // ---- Name resolution (E02xx) ----
    /// A name was used but never declared in scope.
    UnresolvedName,
    /// Two top-level items share a name.
    DuplicateDefinition,
    /// A function declares the same parameter name twice.
    DuplicateParameter,

    // ---- Type checking (E03xx) ----
    /// An expression's type did not match the expected type.
    TypeMismatch,
    /// A call passed the wrong number of arguments.
    ArityMismatch,
    /// A non-function value was called.
    NotCallable,
    /// A `return` value disagreed with the function's return type.
    ReturnTypeMismatch,
    /// An `if`/`while` condition was not `bool`.
    NonBoolCondition,
    /// An operator was applied to operands of incompatible type.
    InvalidOperands,
    /// A function with a non-`unit` return type can fall off its end.
    MissingReturn,
    /// Assignment to an immutable binding.
    AssignToImmutable,
    /// The program has no `main` function, or `main` has the wrong signature.
    BadMain,
    /// `if` or `match` branches with incompatible types, or an `if` without
    /// `else` whose block is not `unit`.
    IfBranchMismatch,
    /// A type annotation named a type that does not exist.
    UnknownType,
    /// The left-hand side of an assignment is not an assignable place.
    InvalidAssignTarget,
    /// `break` or `continue` used outside of any loop.
    BreakOutsideLoop,
    /// A `const` initialiser is not a compile-time constant expression.
    NotConstant,
    /// An array element type that is not a primitive, or an empty array
    /// literal.
    BadArrayType,
    /// Indexing, or `for … in`, applied to a value that is not an array.
    NotIndexable,
    /// A struct field or tuple element that does not exist, or `.` on a value
    /// that has neither.
    UnknownField,
    /// A struct literal that omits or repeats a field or names a non-struct, or
    /// a struct declaring a field twice.
    BadStructLiteral,
    /// A `match` whose arms do not cover every possible value of the scrutinee.
    NonExhaustiveMatch,
}

impl DiagCode {
    /// Every code, in numeric order.
    pub const ALL: [DiagCode; 30] = {
        use DiagCode::*;
        [
            UnexpectedChar,
            UnterminatedString,
            InvalidNumber,
            UnterminatedComment,
            InvalidEscape,
            UnexpectedToken,
            UnexpectedEof,
            ExpectedType,
            UnresolvedName,
            DuplicateDefinition,
            DuplicateParameter,
            TypeMismatch,
            ArityMismatch,
            NotCallable,
            ReturnTypeMismatch,
            NonBoolCondition,
            InvalidOperands,
            MissingReturn,
            AssignToImmutable,
            BadMain,
            IfBranchMismatch,
            UnknownType,
            InvalidAssignTarget,
            BreakOutsideLoop,
            NotConstant,
            BadArrayType,
            NotIndexable,
            UnknownField,
            BadStructLiteral,
            NonExhaustiveMatch,
        ]
    };

    /// The canonical string form, e.g. `"E0301"`.
    pub fn as_str(self) -> &'static str {
        use DiagCode::*;
        match self {
            UnexpectedChar => "E0001",
            UnterminatedString => "E0002",
            InvalidNumber => "E0003",
            UnterminatedComment => "E0004",
            InvalidEscape => "E0005",

            UnexpectedToken => "E0100",
            UnexpectedEof => "E0101",
            ExpectedType => "E0102",

            UnresolvedName => "E0200",
            DuplicateDefinition => "E0201",
            DuplicateParameter => "E0202",

            TypeMismatch => "E0300",
            ArityMismatch => "E0301",
            NotCallable => "E0302",
            ReturnTypeMismatch => "E0303",
            NonBoolCondition => "E0304",
            InvalidOperands => "E0305",
            MissingReturn => "E0306",
            AssignToImmutable => "E0307",
            BadMain => "E0308",
            IfBranchMismatch => "E0309",
            UnknownType => "E0310",
            InvalidAssignTarget => "E0311",
            BreakOutsideLoop => "E0312",
            NotConstant => "E0313",
            BadArrayType => "E0314",
            NotIndexable => "E0315",
            UnknownField => "E0316",
            BadStructLiteral => "E0317",
            NonExhaustiveMatch => "E0318",
        }
    }
}

impl std::fmt::Display for DiagCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No two variants share a code string.
    #[test]
    fn codes_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for code in DiagCode::ALL {
            assert!(
                seen.insert(code.as_str()),
                "duplicate code {}",
                code.as_str()
            );
        }
    }

    /// `ALL` is in code order.
    #[test]
    fn all_is_complete_and_sorted() {
        let codes: Vec<&str> = DiagCode::ALL.iter().map(|c| c.as_str()).collect();
        let mut sorted = codes.clone();
        sorted.sort_unstable();
        assert_eq!(codes, sorted, "DiagCode::ALL is not in code order");
    }
}

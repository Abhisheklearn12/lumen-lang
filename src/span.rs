//! Byte spans into the source file.
//!
//! A compilation has exactly one source file (Lumen has no modules), so a
//! [`Span`] needs no file id. Offsets are `u32`: 4 GiB of source is plenty, and
//! it keeps `Span` at 8 bytes, which matters because every AST and HIR node
//! carries one.

use std::fmt;

/// A half-open byte range `[lo, hi)`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    /// Inclusive start offset.
    pub lo: u32,
    /// Exclusive end offset.
    pub hi: u32,
}

impl Span {
    /// Creates a span. An inverted range is a compiler bug, so it is
    /// debug-asserted.
    #[inline]
    pub fn new(lo: u32, hi: u32) -> Span {
        debug_assert!(lo <= hi, "inverted span: {lo}..{hi}");
        Span { lo, hi }
    }

    /// An empty span at offset 0, for nodes with no real source location.
    pub const DUMMY: Span = Span { lo: 0, hi: 0 };

    /// Length in bytes.
    #[inline]
    pub fn len(&self) -> u32 {
        self.hi - self.lo
    }

    /// Whether the span covers no bytes.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.lo == self.hi
    }

    /// The smallest span covering both `self` and `other`.
    #[inline]
    pub fn to(self, other: Span) -> Span {
        Span::new(self.lo.min(other.lo), self.hi.max(other.hi))
    }

    /// The empty span at `self.lo`.
    #[inline]
    pub fn shrink_to_lo(self) -> Span {
        Span::new(self.lo, self.lo)
    }

    /// The empty span at `self.hi`.
    #[inline]
    pub fn shrink_to_hi(self) -> Span {
        Span::new(self.hi, self.hi)
    }

    /// The span as a `usize` range, for slicing source text.
    #[inline]
    pub fn range(&self) -> std::ops::Range<usize> {
        self.lo as usize..self.hi as usize
    }
}

impl fmt::Debug for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}..{}", self.lo, self.hi)
    }
}

/// A value paired with the span it came from.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Spanned<T> {
    pub node: T,
    pub span: Span,
}

impl<T> Spanned<T> {
    #[inline]
    pub fn new(node: T, span: Span) -> Spanned<T> {
        Spanned { node, span }
    }

    /// Maps the value, keeping the span.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Spanned<U> {
        Spanned {
            node: f(self.node),
            span: self.span,
        }
    }
}

impl<T: fmt::Debug> fmt::Debug for Spanned<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}@{:?}", self.node, self.span)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_covers_both() {
        let a = Span::new(2, 5);
        let b = Span::new(8, 10);
        assert_eq!(a.to(b), Span::new(2, 10));
        assert_eq!(b.to(a), Span::new(2, 10));
    }

    #[test]
    fn len_and_emptiness() {
        assert_eq!(Span::new(3, 7).len(), 4);
        assert!(Span::DUMMY.is_empty());
        assert!(!Span::new(0, 1).is_empty());
    }

    #[test]
    fn shrink_endpoints() {
        let s = Span::new(4, 9);
        assert_eq!(s.shrink_to_lo(), Span::new(4, 4));
        assert_eq!(s.shrink_to_hi(), Span::new(9, 9));
    }
}

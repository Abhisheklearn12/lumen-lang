//! Semantic analysis, between parsing and lowering:
//!
//! * [`types`]: the [`Type`] representation and [`Builtin`] signatures.
//! * [`mod@resolve`]: name resolution.
//! * [`typeck`]: type checking.
//!
//! Both passes read the [`Ast`](crate::parser::ast::Ast) without changing it
//! and record their results in side tables keyed by
//! [`NodeId`](crate::parser::ast::NodeId).

pub mod resolve;
pub mod typeck;
pub mod types;

pub use resolve::{ConstId, FnId, Res, Resolution, StructId, resolve};
pub use typeck::{ConstValue, FnSig, StructInfo, Typeck, check};
pub use types::{Builtin, Elem, Type};

//! Re-exports the [`integer_blossom`] crate's minimum-weight perfect matching
//! solver.
//!
//! The blossom algorithm itself, its `Weight` trait, and its tests live in
//! the standalone `crates/matching` crate (package `integer-blossom`,
//! extracted per TODO.md so it can be reused, benchmarked, or fuzzed
//! independently of `osp-core`). This module just gives [`crate::pairing`] its
//! usual `crate::matching::…` path.

pub(crate) use integer_blossom::{min_weight_perfect_matching, Weight};

// SPDX-License-Identifier: MPL-2.0
//! Copies of the d2-rs-render d2svg ports of D2's lib/geo, lib/label,
//! lib/shape, lib/svg (path.go) and lib/textmeasure Markdown layout, which
//! d2scenebuild consumes but d2-rs-render keeps crate-private.
//! geo, label, shape and svgpath are public for d2-rs-isometric.

#![allow(dead_code, unused_imports)]

pub(crate) mod color;
pub mod geo;
pub mod label;
pub(crate) mod markdown;
pub mod shape;
pub mod svgpath;

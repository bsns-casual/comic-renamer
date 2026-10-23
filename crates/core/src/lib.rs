//! Comic Renamer core. Every body is `todo!()` until its task fills it in;
//! the signatures here are the contract later tasks read instead of prose.

pub mod apply;
pub mod corpus;
pub mod fsops;
pub mod komga_api;
pub mod mapping;
pub mod normalize;
pub mod overrides;
pub mod plan;
pub mod preset;
pub mod residue;
pub mod resolve;
pub mod scan;
pub mod title;
pub mod types;
pub mod undo;

pub use types::*;

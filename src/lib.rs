//! ponte — bridges LangChain OpenWiki into the pleme-io fleet.
//!
//! Three shigoto Jobs run as one Dag against a target repo:
//! `ContextAssembleJob` -> `InvokeOpenWikiJob` -> `RouteJob`. See each
//! module for its role; `main.rs` wires them together.

pub mod context;
pub mod openwiki;
pub mod route;
pub mod state;

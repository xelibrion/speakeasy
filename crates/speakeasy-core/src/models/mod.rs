//! Model catalog, storage, and download.

pub mod catalog;
pub mod manager;

pub use catalog::{find, ModelId, ModelInfo, CATALOG};
pub use manager::ModelManager;

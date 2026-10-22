//! Configuration for the resource sampler.

mod types;

pub use types::{
    ResourcesConfig, DEFAULT_RESOURCES_UPDATE_MS, MAX_RESOURCES_UPDATE_MS, MIN_RESOURCES_UPDATE_MS,
};

#[cfg(test)]
mod tests;

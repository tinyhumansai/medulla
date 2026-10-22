//! Bounded, cheaply cloneable sample history.

use std::collections::VecDeque;
use std::sync::Arc;

/// A bounded FIFO ring. Clones share storage until either copy is mutated.
#[derive(Debug, Clone)]
pub struct History<T>(Arc<VecDeque<T>>);

impl<T> Default for History<T> {
    fn default() -> Self {
        Self(Arc::new(VecDeque::new()))
    }
}

impl<T: Clone> History<T> {
    /// Create an empty ring retaining at most `capacity` samples.
    pub fn new(capacity: usize) -> Self {
        Self(Arc::new(VecDeque::with_capacity(capacity)))
    }

    /// Append a sample and discard the oldest entries beyond capacity.
    pub fn push(&mut self, value: T, capacity: usize) {
        let values = Arc::make_mut(&mut self.0);
        if capacity == 0 {
            values.clear();
            return;
        }
        values.push_back(value);
        while values.len() > capacity {
            values.pop_front();
        }
    }

    /// Samples oldest first.
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.0.iter()
    }

    /// Number of retained samples.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether this history has no samples.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use num_traits::Bounded;
use serde::{Deserialize, Serialize};

use crate::ord::Max;
use crate::Semilattice as _;

/// Lamport clock.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Lamport {
    counter: Max<u64>,
}

impl Lamport {
    /// Return the clock value.
    pub fn get(&self) -> u64 {
        *self.counter.get()
    }

    /// Increment clock and return new value.
    /// Must be called before sending a message.
    pub fn tick(&mut self) -> Self {
        self.counter.incr();
        *self
    }

    /// Merge clock with another clock, and increment value.
    /// Must be called whenever a message is received.
    pub fn merge(&mut self, other: Self) -> Self {
        self.counter.merge(other.counter);
        self.tick()
    }

    /// Reset clock to default state.
    pub fn reset(&mut self) {
        self.counter = Max::default();
    }
}

impl From<u64> for Lamport {
    fn from(counter: u64) -> Self {
        Self {
            counter: Max::from(counter),
        }
    }
}

impl Bounded for Lamport {
    fn min_value() -> Self {
        Self::from(u64::min_value())
    }

    fn max_value() -> Self {
        Self::from(u64::max_value())
    }
}

/// Physical clock. Tracks real-time by the second.
#[derive(Debug, Default, Copy, Clone, PartialOrd, PartialEq, Ord, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Physical {
    seconds: u64,
}

impl Physical {
    pub fn new(seconds: u64) -> Self {
        Self { seconds }
    }

    pub fn now() -> Self {
        #[allow(clippy::unwrap_used)] // Safe because Unix was already invented!
        let duration = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();

        Self {
            seconds: duration.as_secs(),
        }
    }

    pub fn as_secs(&self) -> u64 {
        self.seconds
    }
}

impl From<u64> for Physical {
    fn from(seconds: u64) -> Self {
        Self { seconds }
    }
}

impl std::ops::Add<u64> for Physical {
    type Output = Self;

    fn add(self, rhs: u64) -> Self::Output {
        Self {
            seconds: self.seconds + rhs,
        }
    }
}

impl Bounded for Physical {
    fn min_value() -> Self {
        Self {
            seconds: u64::min_value(),
        }
    }

    fn max_value() -> Self {
        Self {
            seconds: u64::max_value(),
        }
    }
}

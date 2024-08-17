//! Bitset implementation.
//!
//! Bitsets are powered by the [`bitset_core`] crate.
//!
//! [`bitset_core`]: https://docs.rs/bitset_core

use std::ops::RangeBounds;

use crate::prelude::*;

// 2^32 gives  4 billion concurrent entities for 512MB   of ram per component
// 2^24 gives 16 million concurrent entities for 2MB     of ram per component
// 2^20 gives  1 million concurrent entities for 128KB   of ram per component
// 2^16 gives 65536      concurrent entities for 8KB     of ram per component
// 2^12 gives 4096       concurrent entities for 512B    of ram per component
// SIMD processes 256 bits/entities (32 bytes) at once when comparing bitsets.
#[cfg(feature = "keysize16")]
const BITSET_EXP: u32 = 16;
#[cfg(all(
    feature = "keysize20",
    not(feature = "keysize16"),
    not(feature = "keysize24"),
    not(feature = "keysize32")
))]
const BITSET_EXP: u32 = 20;
#[cfg(all(
    feature = "keysize24",
    not(feature = "keysize16"),
    not(feature = "keysize20"),
    not(feature = "keysize32")
))]
const BITSET_EXP: u32 = 24;
#[cfg(all(
    feature = "keysize32",
    not(feature = "keysize16"),
    not(feature = "keysize20"),
    not(feature = "keysize24")
))]
const BITSET_EXP: u32 = 32;

pub use bitset_core::*;
use roaring::RoaringBitmap;

pub(crate) const BITSET_SIZE: usize = 2usize.saturating_pow(BITSET_EXP);
pub(crate) const BITSET_SLICE_COUNT: usize = BITSET_SIZE / (32 * 8 / 8);

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Clone, Debug, Default)]
pub struct BitSetVec(pub RoaringBitmap);

impl BitSetVec {
    /// Returns the maximum value in the set (if the set is non-empty).
    pub fn max(&self) -> Option<u32> {
        self.0.max()
    }

    /// Find the minimum value that is not set in the bitmap.
    pub fn first_free(&self, start: u32) -> u32 {
        let mut current = start;
        for value in self.0.iter() {
            if value != current {
                return current;
            }
            current = value + 1;
        }
        current
    }

    /// Check whether or not the bitset contains the given entity.
    pub fn contains(&self, entity: Entity) -> bool {
        self.0.contains(entity.index())
    }

    /// Insert an entity into the bitset.
    pub fn insert(&mut self, entity: Entity) {
        self.0.insert(entity.index());
    }

    /// Remove an entity from the bitset.
    pub fn remove(&mut self, entity: Entity) {
        self.0.remove(entity.index());
    }

    /// Clear the bitset.
    pub fn clear(&mut self) {
        self.0.clear();
    }

    /// Get the number of entities in the bitset.
    pub fn len(&self) -> usize {
        self.0.len() as usize
    }

    /// Check if the bitset is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Apply a bitwise AND operation with another bitset.
    #[inline]
    pub fn apply_bitset(&mut self, other: &BitSetVec) {
        self.0 &= &other.0;
    }

    /// Apply a bitwise AND operation with another bitset.
    #[inline]
    pub fn bit_and(&mut self, other: &BitSetVec) {
        self.0 &= &other.0;
    }

    /// Test if a bit is set.
    #[inline]
    pub fn bit_test(&self, index: usize) -> bool {
        self.0.contains(index as u32)
    }

    /// Set a bit.
    #[inline]
    pub fn bit_set(&mut self, index: usize) {
        self.0.insert(index as u32);
    }

    /// Reset (unset) a bit.
    #[inline]
    pub fn bit_reset(&mut self, index: usize) {
        self.0.remove(index as u32);
    }

    /// Get the length of the bitset (maximum value + 1).
    #[inline]
    pub fn bit_len(&self) -> usize {
        self.0.max().map_or(0, |max| max as usize + 1)
    }

    /// Find the next set bit starting from the given index.
    #[inline]
    pub fn next_set_bit(&self, start: usize) -> Option<usize> {
        self.0.min().map(|x| x as usize)
    }
}

/// Creates an empty RoaringBitmap.
pub fn create_bitset() -> BitSetVec {
    BitSetVec(RoaringBitmap::new())
}

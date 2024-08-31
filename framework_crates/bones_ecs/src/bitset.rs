//! Bitset implementation.
//!
//! Bitsets are powered by the [`bitset_core`] crate.
//!
//! [`bitset_core`]: https://docs.rs/bitset_core

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

pub(crate) const BITSET_SIZE: usize = 2usize.saturating_pow(BITSET_EXP);
pub(crate) const BITSET_SLICE_COUNT: usize = BITSET_SIZE / (32 * 8 / 8);

/// The type of bitsets used to track entities in component storages.
/// Mostly used to create caches.
#[derive(Deref, DerefMut, Clone, Debug)]
pub struct BitSetVec(pub Vec<[u32; 8]>);

impl Default for BitSetVec {
    fn default() -> Self {
        create_bitset()
    }
}

impl BitSetVec {
    /// Check whether or not the bitset contains the given entity.
    #[inline]
    pub fn contains(&self, entity: Entity) -> bool {
        self.bit_test(entity.index() as usize)
    }
}

/// Creates a bitset big enough to contain the index of each entity.
/// Mostly used to create caches.
pub fn create_bitset() -> BitSetVec {
    BitSetVec(vec![[0u32; 8]; BITSET_SLICE_COUNT])
}

#[cfg(feature = "serde")]
mod serde_impl {
    use super::*;
    use serde::de::{SeqAccess, Visitor};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::fmt;

    impl Serialize for BitSetVec {
        fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            use serde::ser::SerializeSeq;

            let non_zero: Vec<_> = self
                .0
                .iter()
                .enumerate()
                .filter(|(_, arr)| arr.iter().any(|&x| x != 0))
                .collect();

            let mut seq = serializer.serialize_seq(Some(non_zero.len()))?;
            for (idx, arr) in non_zero {
                seq.serialize_element(&(idx, arr))?;
            }
            seq.end()
        }
    }

    impl<'de> Deserialize<'de> for BitSetVec {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
        {
            struct BitSetVecVisitor;

            impl<'de> Visitor<'de> for BitSetVecVisitor {
                type Value = BitSetVec;

                fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                    formatter.write_str("a sequence of (index, array) pairs")
                }

                fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
                where
                    A: SeqAccess<'de>,
                {
                    let mut vec = vec![[0u32; 8]; BITSET_SLICE_COUNT];

                    while let Some((idx, arr)) = seq.next_element::<(usize, [u32; 8])>()? {
                        if idx < BITSET_SLICE_COUNT {
                            vec[idx] = arr;
                        }
                    }

                    Ok(BitSetVec(vec))
                }
            }

            deserializer.deserialize_seq(BitSetVecVisitor)
        }
    }
}

// Helper function to create a BitSetVec for testing
fn create_test_bitsetvec() -> BitSetVec {
    let mut bsv = BitSetVec(vec![[0; 8]; 3]);
    bsv.0[1] = [1, 2, 3, 4, 5, 6, 7, 8];
    bsv
}

#[cfg(all(test, feature = "serde"))]
mod tests {
    use super::*;
    use serde_json;

    #[test]
    fn test_serialization() {
        let bsv = create_test_bitsetvec();
        let serialized = serde_json::to_string(&bsv).unwrap();
        assert_eq!(serialized, "[[1,[1,2,3,4,5,6,7,8]]]");
    }

    #[test]
    fn test_deserialization() {
        let json = "[[1,[1,2,3,4,5,6,7,8]]]";
        let deserialized: BitSetVec = serde_json::from_str(json).unwrap();
        assert_eq!(deserialized.0.len(), 2);
        assert_eq!(deserialized.0[0], [0; 8]);
        assert_eq!(deserialized.0[1], [1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn test_empty_serialization() {
        let empty_bsv = BitSetVec(vec![]);
        let serialized = serde_json::to_string(&empty_bsv).unwrap();
        assert_eq!(serialized, "[]");
    }

    #[test]
    fn test_empty_deserialization() {
        let json = "[]";
        let deserialized: BitSetVec = serde_json::from_str(json).unwrap();
        assert_eq!(deserialized.0.len(), 0);
    }

    #[test]
    fn test_sparse_bitsetvec_serialization_deserialization() {
        let mut bsv = BitSetVec::default();

        // Set some bits sparsely
        bsv.bit_set(5);
        bsv.bit_set(10);
        bsv.bit_set(300); // This will be in the second chunk
        bsv.bit_set(1000); // This will be in the fourth chunk

        // Serialize
        let serialized = serde_json::to_string(&bsv).unwrap();

        // Deserialize
        let deserialized: BitSetVec = serde_json::from_str(&serialized).unwrap();

        // Check if all set bits are correctly deserialized
        assert!(deserialized.bit_test(5));
        assert!(deserialized.bit_test(10));
        assert!(deserialized.bit_test(300));
        assert!(deserialized.bit_test(1000));

        // Check some unset bits
        assert!(!deserialized.bit_test(0));
        assert!(!deserialized.bit_test(6));
        assert!(!deserialized.bit_test(11));
        assert!(!deserialized.bit_test(299));
        assert!(!deserialized.bit_test(301));
        assert!(!deserialized.bit_test(999));
        assert!(!deserialized.bit_test(1001));

        // Check if the internal structure matches
        assert_eq!(bsv.0.len(), deserialized.0.len());
        for (original, deserialized) in bsv.0.iter().zip(deserialized.0.iter()) {
            assert_eq!(original, deserialized);
        }

        // Print the serialized string for inspection
        println!("Serialized BitSetVec: {}", serialized);
    }

    #[test]
    fn test_size() {
        let mut bitset = BitSetVec::default();
        bitset.bit_set(5);
        bitset.bit_set(10);
        bitset.bit_set(300);
        bitset.bit_set(1000);

        let bytes = bincode::serialize(&bitset).unwrap();
        println!("Size of serialized: {}", bytes.len());
        println!("Original size: {} bytes", std::mem::size_of_val(&bitset));
        let _: BitSetVec = bincode::deserialize(&bytes).unwrap();
    }
}

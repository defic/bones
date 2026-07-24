//! Simple reflection system based on the `#[repr(C)]` memory layout.
//!
//! You can derive [`HasSchema`] for your Rust types to unlock integration with the `bones_schema`
//! ecosystem, including `bones_ecs` and `bones_asset`.

#![warn(missing_docs)]
// This cfg_attr is needed because `rustdoc::all` includes lints not supported on stable
#![cfg_attr(doc, allow(unknown_lints))]
#![deny(rustdoc::all)]
// This allows us to use our stable polyfills for nightly APIs under the same name.
#![allow(unstable_name_collisions)]

// import the macros if the derive feature is enabled.
#[cfg(feature = "derive")]
pub use bones_schema_macros::*;

/// The prelude.
pub mod prelude {
    #[cfg(feature = "serde")]
    pub use crate::ser_de::*;
    pub use crate::{
        alloc::{SMap, SVec, SchemaMap, SchemaVec},
        ptr::*,
        registry::*,
        schema::*,
    };
    #[cfg(feature = "derive")]
    pub use bones_schema_macros::*;
    pub use bones_utils;
}

mod schema;
pub use schema::*;

pub mod alloc;
pub mod ptr;
pub mod raw_fns;
pub mod registry;

/// Implementations of [`HasSchema`] for standard types.
mod std_impls;

/// Serde implementations for [`Schema`].
#[cfg(feature = "serde")]
pub mod ser_de;

#[cfg(test)]
mod test {
    #[cfg(feature = "derive")]
    mod derive_test {
        #![allow(dead_code)]

        use crate::prelude::*;

        #[derive(HasSchema, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Debug)]
        #[schema_module(crate)]
        #[repr(C, u8)]
        pub enum Maybe<T> {
            /// The value is not set.
            #[default]
            Unset,
            /// The value is set.
            Set(T),
        }

        #[derive(HasSchema, Clone, Copy, Debug, PartialEq, Eq, Default)]
        #[schema_module(crate)]
        #[repr(u8)]
        pub enum E {
            #[default]
            None,
            L,
            R,
            U,
            D,
            G,
            S,
        }

        /// Represents the ball in the game
        #[derive(HasSchema, Clone, Default)]
        #[schema_module(crate)]
        pub struct A {
            pub c: Maybe<u32>,
            pub d: SVec<u64>,
            pub e: Maybe<u64>,
            pub f: u32,
            pub g: f32,
            pub h: f32,
            pub i: E,
            pub j: u32,
            pub k: u32,
        }

        #[derive(HasSchema, Clone, Default)]
        #[schema_module(crate)]
        #[repr(C)]
        pub struct B {
            pub c: Maybe<u32>,
            pub d: SVec<u64>,
            pub e: Maybe<u64>,
            pub f: u32,
            pub g: f32,
            pub h: f32,
            pub i: E,
            pub j: u32,
            pub k: u32,
        }

        #[derive(HasSchema, Clone, Default)]
        #[schema_module(crate)]
        pub struct C {
            pub c: Maybe<u32>,
            pub e: Maybe<u64>,
        }

        #[derive(HasSchema, Clone, Default)]
        #[schema_module(crate)]
        #[repr(C)]
        pub struct D {
            pub c: Maybe<u32>,
            pub e: Maybe<u64>,
        }

        #[derive(HasSchema, Clone)]
        #[schema(no_default)]
        #[schema_module(crate)]
        #[repr(C)]
        struct F<T> {
            a: bool,
            b: T,
        }

        /// Makes sure that the layout reported in the schema for a generic type matches the layout
        /// reported by Rust, for two different type parameters.
        #[test]
        fn generic_type_schema_layouts_match() {
            assert_eq!(
                Maybe::<u32>::schema().layout(),
                std::alloc::Layout::new::<Maybe<u32>>()
            );
            assert_eq!(
                Maybe::<u64>::schema().layout(),
                std::alloc::Layout::new::<Maybe<u64>>()
            );
            assert_eq!(
                F::<u64>::schema().layout(),
                std::alloc::Layout::new::<F<u64>>()
            );
            assert_eq!(
                F::<u32>::schema().layout(),
                std::alloc::Layout::new::<F<u32>>()
            );

            // Check a normal enum too, just in case.
            assert_eq!(E::schema().layout(), std::alloc::Layout::new::<E>());
        }

        // Makes sure that the layout reported for two structs, where the only difference between
        // them is the `#[repr(C)]` annotation, matches.
        #[test]
        fn schema_layout_for_repr_c_matches_repr_rust() {
            assert_eq!(A::schema().layout(), B::schema().layout());
            assert_eq!(C::schema().layout(), D::schema().layout());
        }

        /// A `GameInput`-shaped payload enum: named-field variants, one holding a heap value
        /// (`SVec`) so `set_variant`'s drop-the-old-payload path is exercised for real.
        #[derive(HasSchema, Clone, Debug, PartialEq)]
        #[schema_module(crate)]
        #[repr(C, u8)]
        pub enum Verb {
            Damage { victim: u32, amount: u32 },
            SetMode { kind: u8 },
            Tag { items: SVec<u64> },
        }
        impl Default for Verb {
            fn default() -> Self {
                Verb::Damage { victim: 0, amount: 0 }
            }
        }

        /// `set_variant` writes the tag, default-initializes the new payload, and the reflected
        /// value round-trips back to the real Rust enum.
        #[test]
        fn enum_set_variant_switches_and_defaults_payload() {
            let mut v = Verb::default();
            {
                let r = crate::ptr::SchemaRefMut::new(&mut v);
                let crate::ptr::SchemaRefMutAccess::Enum(mut e) = r.into_access_mut() else {
                    panic!("not an enum access");
                };
                assert_eq!(e.variant_name(), "Damage");
                assert!(e.set_variant("SetMode"), "known variant must switch");
                assert_eq!(e.variant_name(), "SetMode");
                assert!(!e.set_variant("Nope"), "unknown variant must return false");
                assert_eq!(e.variant_name(), "SetMode", "failed switch must not disturb");
            }
            assert_eq!(v, Verb::SetMode { kind: 0 }, "payload default-initialized");
        }

        /// Field access on an enum ref resolves through the CURRENT variant — read and write —
        /// which is what lets Lua do `gi.victim = …` without naming the variant.
        #[test]
        fn enum_field_access_resolves_through_current_variant() {
            let mut v = Verb::Damage { victim: 7, amount: 9 };
            // Read through the shared ref.
            {
                let r = crate::ptr::SchemaRef::new(&v);
                let victim = r.field("victim").expect("field via current variant");
                assert_eq!(*victim.cast::<u32>(), 7);
                assert!(r.field("kind").is_none(), "other variant's field must miss");
            }
            // Write through the mut ref.
            {
                let r = crate::ptr::SchemaRefMut::new(&mut v);
                let mut amount = r
                    .into_field("amount")
                    .ok()
                    .expect("mut field via current variant");
                *amount.cast_mut::<u32>() = 42;
            }
            assert_eq!(v, Verb::Damage { victim: 7, amount: 42 });
        }

        /// Switching AWAY from a heap-holding variant must drop its payload (no leak, no
        /// double-free when the enum itself drops later), and switching INTO it must yield a
        /// usable default (empty vec) — the field-by-field drop/default-init contract.
        #[test]
        fn enum_set_variant_drops_old_heap_payload() {
            let mut v = Verb::Tag { items: [1u64, 2, 3].into_iter().collect() };
            {
                let r = crate::ptr::SchemaRefMut::new(&mut v);
                let crate::ptr::SchemaRefMutAccess::Enum(mut e) = r.into_access_mut() else {
                    panic!("not an enum access");
                };
                e.set_variant("Damage");
                e.set_variant("Tag");
            }
            assert_eq!(
                v,
                Verb::Tag { items: SVec::default() },
                "re-entered heap variant starts default-empty"
            );
        }
    }
}

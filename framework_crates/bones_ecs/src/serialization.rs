//! Serialization and deserialization of an entire [`World`].
//!
//! Enabled by the `serde` feature. `World` implements [`serde::Serialize`] and
//! [`serde::Deserialize`], so you choose the wire format yourself.
//!
//! # Design
//!
//! See `docs/adr/0001-world-serialization-wire-format.md` and
//! `docs/adr/0002-schema-warmup-and-registration-contract.md`. In short:
//!
//! - Component stores and resources are keyed by their schema **`full_name`** (a stable, portable
//!   identity; the numeric `SchemaId` is registration-order-dependent and must not be used).
//! - Encoding is **sparse**: components emit only `(entity_index, value)` pairs for set bits, and
//!   [`Entities`] uses its own compact form. No dense, mostly-empty buffers are written.
//! - Output is **canonical**: stores/resources are sorted by `full_name` and entities by index, so
//!   identical logical state yields identical bytes (hashable for desync detection).
//! - Each component/resource *value* is encoded with the existing schema walker
//!   ([`SchemaSerializer`]/[`SchemaDeserializer`]).
//!
//! # Requirements
//!
//! - Every component/resource type must be serializable by the schema walker (i.e. `#[repr(C)]` or
//!   carrying serde type-data). An opaque/unserializable type is a hard error at serialize time,
//!   never silently skipped.
//! - Before deserializing, every type in the snapshot must already be registered in
//!   `SCHEMA_REGISTRY`. The natural way to guarantee this is to construct the sim systems first
//!   (their parameters reference every sim type, registering it). A `full_name` that cannot be
//!   resolved is a hard, named error.
//! - The schema walker's deserializer uses `deserialize_any` for structs/enums, so the chosen wire
//!   format must be **self-describing** (e.g. JSON, YAML, MessagePack/CBOR). Non-self-describing
//!   formats like postcard/bincode are not supported by the value codec.

use crate::prelude::*;

use bones_schema::registry::SCHEMA_REGISTRY;
use serde::de::{DeserializeSeed, Error as _, MapAccess, SeqAccess, Visitor};
use serde::ser::{Error as _, SerializeMap, SerializeSeq, SerializeStruct};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

const WORLD_FIELDS: &[&str] = &["entities", "resources", "components"];

/// Look up a registered schema by its `full_name`.
///
/// Returns `None` if no type with that `full_name` has been registered in this process yet.
fn lookup_schema(full_name: &str) -> Option<&'static Schema> {
    SCHEMA_REGISTRY
        .schemas
        .iter()
        .find(|schema| schema.full_name.as_ref() == full_name)
}

/// Whether a type opts out of World serialization via the [`SkipSerialize`] type-data marker.
fn is_skipped(schema: &'static Schema) -> bool {
    schema.type_data.get::<SkipSerialize>().is_some()
}

// ===========================================================================================
//  Serialize
// ===========================================================================================

impl Serialize for World {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // --- Entities (compact special case) ---
        let entities = self.resource::<Entities>();

        // --- Resources: populated, non-Entities cells, sorted by full_name ---
        let mut resource_cells: Vec<AtomicUntypedResource> = self
            .resources
            .untyped()
            .cells()
            .into_iter()
            .filter(|cell| {
                cell.schema() != Entities::schema()
                    && !is_skipped(cell.schema())
                    && cell.borrow().is_some()
            })
            .collect();
        resource_cells.sort_by(|a, b| a.schema().full_name.as_ref().cmp(b.schema().full_name.as_ref()));
        check_no_duplicates::<S>(resource_cells.iter().map(|c| c.schema()), "resource")?;

        // --- Components: non-empty stores, sorted by full_name ---
        let mut store_cells: Vec<UntypedAtomicComponentStore> = self
            .components
            .cells()
            .into_iter()
            .filter(|cell| {
                let store = cell.borrow();
                store.bitset().bit_count() > 0 && !is_skipped(store.schema())
            })
            .collect();
        store_cells.sort_by(|a, b| {
            a.borrow()
                .schema()
                .full_name
                .as_ref()
                .cmp(b.borrow().schema().full_name.as_ref())
        });
        check_no_duplicates::<S>(store_cells.iter().map(|c| c.borrow().schema()), "component")?;

        let mut state = serializer.serialize_struct("World", 3)?;
        state.serialize_field("entities", &*entities)?;
        state.serialize_field("resources", &ResourcesSer(&resource_cells))?;
        state.serialize_field("components", &ComponentsSer(&store_cells))?;
        state.end()
    }
}

/// Error if two schemas in `schemas` share a `full_name` (e.g. distinct generic instantiations,
/// which are not supported). Turns a silent save-corruption hazard into a loud, located failure.
fn check_no_duplicates<'a, S: Serializer>(
    schemas: impl Iterator<Item = &'a Schema>,
    kind: &str,
) -> Result<(), S::Error> {
    let mut names: Vec<&str> = schemas.map(|s| s.full_name.as_ref()).collect();
    // `names` arrives already sorted, but sort defensively in case a caller changes.
    names.sort_unstable();
    for pair in names.windows(2) {
        if pair[0] == pair[1] {
            return Err(S::Error::custom(format!(
                "cannot serialize World: two {kind} types share the full_name `{}`. Generic ECS \
                 types are not supported for serialization because their type parameters are not \
                 part of the full_name.",
                pair[0]
            )));
        }
    }
    Ok(())
}

struct ResourcesSer<'a>(&'a [AtomicUntypedResource]);
impl Serialize for ResourcesSer<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for cell in self.0 {
            let borrow = cell.borrow();
            // SOUND: cells were filtered to populated ones before being collected.
            let value = borrow.as_ref().expect("resource cell unexpectedly empty");
            map.serialize_entry(
                cell.schema().full_name.as_ref(),
                &SchemaSerializer(value.as_ref()),
            )?;
        }
        map.end()
    }
}

struct ComponentsSer<'a>(&'a [UntypedAtomicComponentStore]);
impl Serialize for ComponentsSer<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for cell in self.0 {
            let store = cell.borrow();
            map.serialize_entry(store.schema().full_name.as_ref(), &StoreSer(&store))?;
        }
        map.end()
    }
}

struct StoreSer<'a>(&'a UntypedComponentStore);
impl Serialize for StoreSer<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(Some(self.0.bitset().bit_count()))?;
        for (index, value) in self.0.iter_indexed() {
            seq.serialize_element(&(index, SchemaSerializer(value)))?;
        }
        seq.end()
    }
}

// ===========================================================================================
//  Deserialize
// ===========================================================================================

/// Deserialize a whole [`World`] from a self-describing format.
///
/// # Warning: register your types first
///
/// Every component and resource type in the snapshot must already be registered in the global
/// schema registry **before** you deserialize, or this fails with a "type ... is not registered"
/// error. Schemas register lazily (on the first `T::schema()` call) and a `full_name` string on the
/// wire cannot rebuild a type — so a type the client has never touched is invisible.
///
/// The reliable way to satisfy this is to **construct your simulation systems first**: their
/// parameters (`Comp<T>`, `Res<T>`, …) reference every sim type and register it. Build the sim,
/// then deserialize. See `docs/adr/0002-schema-warmup-and-registration-contract.md`.
impl<'de> Deserialize<'de> for World {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let world = World::default();
        deserializer.deserialize_struct("World", WORLD_FIELDS, WorldVisitor { world: &world })?;
        Ok(world)
    }
}

#[derive(Clone, Copy)]
struct WorldVisitor<'a> {
    world: &'a World,
}

impl<'a, 'de> Visitor<'de> for WorldVisitor<'a> {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a serialized bones_ecs World")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let entities: Entities = seq
            .next_element()?
            .ok_or_else(|| A::Error::custom("missing `entities`"))?;
        self.world.insert_resource(entities);
        seq.next_element_seed(ResourcesSeed { world: self.world })?
            .ok_or_else(|| A::Error::custom("missing `resources`"))?;
        seq.next_element_seed(ComponentsSeed { world: self.world })?
            .ok_or_else(|| A::Error::custom("missing `components`"))?;
        Ok(())
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        while let Some(field) = map.next_key::<Field>()? {
            match field {
                Field::Entities => {
                    let entities: Entities = map.next_value()?;
                    self.world.insert_resource(entities);
                }
                Field::Resources => {
                    map.next_value_seed(ResourcesSeed { world: self.world })?;
                }
                Field::Components => {
                    map.next_value_seed(ComponentsSeed { world: self.world })?;
                }
            }
        }
        Ok(())
    }
}

enum Field {
    Entities,
    Resources,
    Components,
}

impl<'de> Deserialize<'de> for Field {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct FieldVisitor;
        impl Visitor<'_> for FieldVisitor {
            type Value = Field;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("one of `entities`, `resources`, `components`")
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Field, E> {
                match v {
                    "entities" => Ok(Field::Entities),
                    "resources" => Ok(Field::Resources),
                    "components" => Ok(Field::Components),
                    _ => Err(E::unknown_field(v, WORLD_FIELDS)),
                }
            }
        }
        deserializer.deserialize_identifier(FieldVisitor)
    }
}

/// Deserializes the `resources` map (`full_name -> value`) directly into `world`.
struct ResourcesSeed<'a> {
    world: &'a World,
}
impl<'a, 'de> DeserializeSeed<'de> for ResourcesSeed<'a> {
    type Value = ();
    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_map(self)
    }
}
impl<'a, 'de> Visitor<'de> for ResourcesSeed<'a> {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a map of resource full_name to value")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        while let Some(full_name) = map.next_key::<String>()? {
            let schema = lookup_schema(&full_name).ok_or_else(|| missing_type::<A>(&full_name))?;
            let value: SchemaBox = map.next_value_seed(SchemaDeserializer(schema))?;
            self.world
                .resources
                .untyped()
                .get(schema)
                .insert(value)
                .map_err(|_| A::Error::custom(format!("schema mismatch for resource `{full_name}`")))?;
        }
        Ok(())
    }
}

/// Deserializes the `components` map (`full_name -> [(index, value)]`) directly into `world`.
struct ComponentsSeed<'a> {
    world: &'a World,
}
impl<'a, 'de> DeserializeSeed<'de> for ComponentsSeed<'a> {
    type Value = ();
    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_map(self)
    }
}
impl<'a, 'de> Visitor<'de> for ComponentsSeed<'a> {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a map of component full_name to entries")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        while let Some(full_name) = map.next_key::<String>()? {
            let schema = lookup_schema(&full_name).ok_or_else(|| missing_type::<A>(&full_name))?;
            map.next_value_seed(StoreSeed {
                world: self.world,
                schema,
            })?;
        }
        Ok(())
    }
}

/// Deserializes a single component store's `[(index, value)]` entries into `world`.
struct StoreSeed<'a> {
    world: &'a World,
    schema: &'static Schema,
}
impl<'a, 'de> DeserializeSeed<'de> for StoreSeed<'a> {
    type Value = ();
    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_seq(self)
    }
}
impl<'a, 'de> Visitor<'de> for StoreSeed<'a> {
    type Value = ();
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a sequence of (entity_index, value) pairs")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        let cell = self.world.components.get_by_schema(self.schema);
        let mut store = cell.borrow_mut();
        while let Some((index, value)) = seq.next_element_seed(PairSeed(self.schema))? {
            store.insert_box(Entity::new(index, 0), value);
        }
        Ok(())
    }
}

/// Deserializes a single `(entity_index, value)` pair, decoding the value via its schema.
struct PairSeed(&'static Schema);
impl<'de> DeserializeSeed<'de> for PairSeed {
    type Value = (u32, SchemaBox);
    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_tuple(2, self)
    }
}
impl<'de> Visitor<'de> for PairSeed {
    type Value = (u32, SchemaBox);
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a (entity_index, value) pair")
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let index: u32 = seq
            .next_element()?
            .ok_or_else(|| A::Error::custom("missing entity index in component entry"))?;
        let value: SchemaBox = seq
            .next_element_seed(SchemaDeserializer(self.0))?
            .ok_or_else(|| A::Error::custom("missing value in component entry"))?;
        Ok((index, value))
    }
}

fn missing_type<'de, A: MapAccess<'de>>(full_name: &str) -> A::Error {
    A::Error::custom(format!(
        "type `{full_name}` from the snapshot is not registered for deserialization; construct \
         your simulation systems (which reference all sim types) before deserializing the World"
    ))
}

#[cfg(test)]
mod tests {
    use crate::prelude::*;

    fn hash(bytes: &[u8]) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        bytes.hash(&mut h);
        h.finish()
    }

    #[derive(HasSchema, Clone, Debug, Default, PartialEq)]
    #[repr(C)]
    struct Pos {
        x: f32,
        y: f32,
    }

    #[derive(HasSchema, Clone, Debug, Default, PartialEq)]
    #[repr(C)]
    struct Vel {
        x: f32,
        y: f32,
    }

    #[derive(HasSchema, Clone, Debug, Default, PartialEq)]
    #[repr(C)]
    struct Health(u32);

    #[derive(HasSchema, Clone, Debug, Default, PartialEq)]
    #[repr(C)]
    struct Score(u64);

    /// Build a world with a few entities, one killed, and a resource.
    fn build_world() -> World {
        let world = World::new();
        world.run_system(
            |mut entities: ResMut<Entities>,
             mut pos: CompMut<Pos>,
             mut vel: CompMut<Vel>,
             mut health: CompMut<Health>| {
                let e0 = entities.create();
                pos.insert(e0, Pos { x: 1.0, y: 2.0 });
                vel.insert(e0, Vel { x: 0.5, y: -0.5 });
                health.insert(e0, Health(100));

                let e1 = entities.create();
                pos.insert(e1, Pos { x: 3.0, y: 4.0 });

                let e2 = entities.create();
                vel.insert(e2, Vel { x: 1.0, y: 1.0 });
                health.insert(e2, Health(50));

                // Kill one so generations diverge from the default.
                entities.kill(e1);
            },
            (),
        );
        world.maintain();
        world.insert_resource(Score(42));
        world
    }

    fn assert_worlds_eq(a: &World, b: &World) {
        // Entities: same alive set + generations.
        let ea = a.resource::<Entities>();
        let eb = b.resource::<Entities>();
        assert_eq!(ea.all_cloned(), eb.all_cloned(), "alive entity sets differ");

        // Components match per entity.
        for entity in ea.all_cloned() {
            assert_eq!(
                a.component::<Pos>().get(entity),
                b.component::<Pos>().get(entity),
                "Pos differs for {entity:?}"
            );
            assert_eq!(
                a.component::<Vel>().get(entity),
                b.component::<Vel>().get(entity),
                "Vel differs for {entity:?}"
            );
            assert_eq!(
                a.component::<Health>().get(entity),
                b.component::<Health>().get(entity),
                "Health differs for {entity:?}"
            );
        }

        // Resource matches.
        assert_eq!(
            a.resource::<Score>().0,
            b.resource::<Score>().0,
            "Score resource differs"
        );
    }

    /// 1. Round-trip equality.
    #[test]
    fn roundtrip_equality() {
        let world = build_world();
        let json = serde_json::to_string(&world).unwrap();
        let restored: World = serde_json::from_str(&json).unwrap();
        assert_worlds_eq(&world, &restored);
    }

    /// 2. Hash stability: two independently-built identical worlds serialize to identical bytes;
    ///    a mutation changes the bytes.
    #[test]
    fn hash_stability() {
        let a = build_world();
        let b = build_world();
        let ja = serde_json::to_vec(&a).unwrap();
        let jb = serde_json::to_vec(&b).unwrap();
        assert_eq!(hash(&ja), hash(&jb), "identical state must hash equal");

        a.insert_resource(Score(43));
        let ja2 = serde_json::to_vec(&a).unwrap();
        assert_ne!(hash(&ja2), hash(&jb), "changed state must hash differently");
    }

    /// 3. Entity identity determinism: after a round-trip, future create()/kill() allocate the same
    ///    indices and generations as the original world would.
    #[test]
    fn entity_identity_determinism() {
        let original = build_world();
        let json = serde_json::to_string(&original).unwrap();
        let restored: World = serde_json::from_str(&json).unwrap();

        let next_original = original.resource_mut::<Entities>().create();
        let next_restored = restored.resource_mut::<Entities>().create();
        assert_eq!(
            next_original, next_restored,
            "restored world must allocate identical entity index+generation"
        );
    }

    /// 4. Compactness: an empty world must not serialize to the large dense buffers.
    #[test]
    fn empty_world_is_compact() {
        let world = World::new();
        let bytes = serde_json::to_vec(&world).unwrap();
        assert!(
            bytes.len() < 1024,
            "empty world serialized to {} bytes; expected a small sparse encoding",
            bytes.len()
        );
    }

    /// 5. Mid-join lockstep: server world -> bytes -> fresh client world, then stepping both with
    ///    identical logic yields identical state and hashes.
    #[test]
    fn mid_join_lockstep() {
        fn step(entities: Res<Entities>, mut pos: CompMut<Pos>, vel: Comp<Vel>) {
            for (_, (pos, vel)) in entities.iter_with((&mut pos, &vel)) {
                pos.x += vel.x;
                pos.y += vel.y;
            }
        }

        let server = build_world();
        let json = serde_json::to_string(&server).unwrap();
        let client: World = serde_json::from_str(&json).unwrap();

        for _ in 0..10 {
            server.run_system(step, ());
            client.run_system(step, ());
        }

        let server_bytes = serde_json::to_vec(&server).unwrap();
        let client_bytes = serde_json::to_vec(&client).unwrap();
        assert_eq!(
            hash(&server_bytes),
            hash(&client_bytes),
            "server and joined client diverged after identical stepping"
        );
    }

    /// 5b. The compact binary wire (MessagePack via rmp-serde) round-trips through the schema
    ///     walker and is far smaller than the JSON form.
    #[test]
    fn messagepack_roundtrip_and_compactness() {
        let world = build_world();

        let mp = rmp_serde::to_vec(&world).unwrap();
        let restored: World = rmp_serde::from_slice(&mp).unwrap();
        assert_worlds_eq(&world, &restored);

        // Canonical bytes: identical state hashes equal in MessagePack too.
        let other = build_world();
        let mp_other = rmp_serde::to_vec(&other).unwrap();
        assert_eq!(hash(&mp), hash(&mp_other), "identical state must hash equal in MessagePack");

        // Empty world stays tiny in the binary form.
        let empty = rmp_serde::to_vec(&World::new()).unwrap();
        assert!(
            empty.len() < 256,
            "empty world MessagePack was {} bytes; expected a small sparse encoding",
            empty.len()
        );
    }

    /// 6a. Serializing an opaque (non-serializable) type errors loudly.
    #[test]
    fn opaque_type_errors() {
        #[derive(HasSchema, Clone)]
        #[schema(opaque, no_default)]
        struct Opaque {
            _inner: std::sync::Arc<()>,
        }
        impl Default for Opaque {
            fn default() -> Self {
                Opaque {
                    _inner: std::sync::Arc::new(()),
                }
            }
        }

        let world = World::new();
        world.insert_resource(Opaque::default());
        let result = serde_json::to_string(&world);
        assert!(result.is_err(), "expected serialization of opaque type to error");
    }

    /// 6c. An opaque type tagged with [`SkipSerialize`] is skipped, not errored, and is simply
    ///     absent from the snapshot (rebuilt locally on the client).
    #[test]
    fn skip_serialize_excludes_opaque() {
        #[derive(HasSchema, Clone)]
        #[schema(no_default)]
        #[type_data(SkipSerialize)]
        struct Runtime {
            _inner: std::sync::Arc<()>,
        }

        let world = build_world();
        world.insert_resource(Runtime {
            _inner: std::sync::Arc::new(()),
        });

        // Would error without the marker; with it, serialization succeeds.
        let json = serde_json::to_string(&world).expect("skipped opaque type must not error");
        assert!(
            !json.contains("Runtime"),
            "skipped type must not appear in the snapshot"
        );

        // Round-trips fine; the skipped resource is absent on the restored world.
        let restored: World = serde_json::from_str(&json).unwrap();
        assert_worlds_eq(&world, &restored);
        assert!(
            restored.get_resource::<Runtime>().is_none(),
            "skipped resource should be absent after deserialize"
        );
    }

    /// 6b. Deserializing a snapshot referencing an unregistered type errors with a clear message.
    #[test]
    fn unregistered_type_errors() {
        let json = r#"{
            "entities": {"next_id":0,"has_deleted":false,"killed":[],"alive":[],"generations":[]},
            "resources": {"some::Unregistered::Type": {"x": 1}},
            "components": {}
        }"#;
        let result: Result<World, _> = serde_json::from_str(json);
        let err = result.expect_err("expected unregistered type to error");
        assert!(
            err.to_string().contains("not registered"),
            "error should explain the type is not registered, got: {err}"
        );
    }
}

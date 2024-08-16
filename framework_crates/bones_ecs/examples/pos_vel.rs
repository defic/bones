use std::borrow::{Borrow, BorrowMut};

use bincode::deserialize;
use bones_ecs::prelude::*;

// Define our component types.
//
// Each component must derive `HasSchema`.

#[derive(Clone, Debug, HasSchema, Default)]
#[repr(C)]
pub struct Vel {
    x: f32,
    y: f32,
}

#[derive(Clone, Debug, HasSchema, Default)]
#[repr(C)]
pub struct Pos {
    x: f32,
    y: f32,
}

fn main() {
    // Initialize an empty world
    let mut world = World::new();

    // Create a SystemStages to store the systems that we will run more than once.
    let mut stages = SystemStages::with_core_stages();

    // Add our systems to the system stages
    stages
        .add_startup_system(startup_system)
        .add_system_to_stage(CoreStage::Update, pos_vel_system)
        .add_system_to_stage(CoreStage::PostUpdate, print_system);

    // Run our game loop for 10 frames
    for _ in 0..10 {
        stages.run(&mut world);
    }

    let vel = world.components.serialize_store::<Vel>().unwrap();
    let vel = ComponentStore::<Vel>::deserialize(&vel);
    let pos = world.components.serialize_store::<Pos>().unwrap();
    let pos = ComponentStore::<Pos>::deserialize(&pos);

    let entities = {
        let asd = world.get_resource::<Entities>().unwrap();
        bincode::serialize(&(*asd)).unwrap()
    };
    let entities: Entities = bincode::deserialize(&entities).unwrap();

    let mut world2 = World::new();
    world2.insert_resource(entities);
    world2.components.borrow_mut().insert(vel);
    world2.components.borrow_mut().insert(pos);

    println!("-------------------------------------");
    for _ in 0..10 {
        stages.run(&mut world2);
        stages.run(&mut world);
    }
}

/// Setup system that spawns two entities with a Pos and a Vel component.
fn startup_system(
    mut entities: ResMut<Entities>,
    mut positions: CompMut<Pos>,
    mut velocities: CompMut<Vel>,
) {
    let ent1 = entities.create();
    positions.insert(ent1, Pos { x: 0., y: 0. });
    velocities.insert(ent1, Vel { x: 3.0, y: 1.0 });

    let ent2 = entities.create();
    positions.insert(ent2, Pos { x: 0., y: 100. });
    velocities.insert(ent2, Vel { x: 0.0, y: -1.0 });
}

/// Update the Pos of all entities with both a Pos and a Vel
fn pos_vel_system(entities: Res<Entities>, mut pos: CompMut<Pos>, vel: Comp<Vel>) {
    println!("pos vel sys");
    for (_, (pos, vel)) in entities.iter_with((&mut pos, &vel)) {
        pos.x += vel.x;
        pos.y += vel.y;
    }
}

#[derive(HasSchema, Debug, Default, Clone)]
#[repr(C)]
struct Component {
    string: String,
    start: String,
}

/// Print the Pos and Vel of every entity
fn print_system(pos: Comp<Pos>, vel: Comp<Vel>, entities: Res<Entities>) {
    println!("=====");

    for entity in entities.iter_with_bitset(entities.bitset()) {
        let pos = pos.get(entity);
        let vel = vel.get(entity);
        println!("{entity:?}: {pos:?} - {vel:?}");
    }

    /*
    for (entity, (pos, vel)) in entities.iter_with((&pos, &vel)) {
        println!("{entity:?}: {pos:?} - {vel:?}");
    }
    */
}

#[test]
fn schemakinds() {
    let schema = Component::schema();

    println!("kind: {:?}", schema.kind);
}

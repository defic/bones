use std::time::Instant;

use bones_ecs::prelude::*;
use nalgebra::Vector2;
use rapier2d::prelude::*;
use rapier_example::{
    debug_renderer::{self, Game},
    physics::{CollisionWorld, PhysicsHandle, RapierUserData, Vec2},
};

fn main() {
    let world = World::new();
    let mut stages = SystemStages::with_core_stages();

    stages
        .add_startup_system(startup_system)
        .add_system_to_stage(CoreStage::PreUpdate, read_inputs)
        .add_system_to_stage(CoreStage::Update, apply_inputs)
        .add_system_to_stage(CoreStage::Update, step_physics_world)
        .add_system_to_stage(CoreStage::Update, update_physics_positions)
        .add_system_to_stage(CoreStage::Update, update_physics_positions2);

    let game = Game::new(world, stages);
    debug_renderer::start(game);
}

#[derive(Clone, Debug, HasSchema, Default, Deref, DerefMut)]
#[repr(C)]
pub struct Pos(Vec2);

#[derive(Clone, Debug, HasSchema, Default, Deref, DerefMut)]
#[repr(C)]
pub struct Vel(Vec2);

fn startup_system(
    mut entities: ResMut<Entities>,
    mut positions: CompMut<Pos>,
    mut velocities: CompMut<Vel>,
    mut handles: CompMut<PhysicsHandle>,
    mut inputs: CompMut<Input>,
    mut collision_world: CollisionWorld,
) {
    collision_world.create_rectangle(
        entities.create(),
        Vec2::new(0.0, 5.0).into(),
        0.5,
        3.5,
        10.0,
    );
    let points: [Point<Real>; 3] = [
        Point::new(-5.0, -5.0),
        Point::new(-5.0, 5.0),
        Point::new(5.0, -5.0),
    ];

    let triangle = ColliderBuilder::convex_hull(&points)
        .unwrap()
        .position(Vector::new(15.0, -3.0).into())
        .rotation(std::f32::consts::FRAC_PI_2);
    collision_world.insert_collider(triangle);

    let triangle = ColliderBuilder::convex_hull(&points)
        .unwrap()
        .position(Vector::new(-15.0, -3.0).into());

    collision_world.insert_collider(triangle);

    for i in 0..20 {
        let ent = entities.create();
        let radius = (i % 6) as f32 * 0.1 + 0.05;
        let handle = collision_world.create_ball(ent, radius, 5.0, Default::default());
        positions.insert(ent, Vec2 { x: 0., y: 0. }.into());
        velocities.insert(ent, Default::default());
        handles.insert(ent, handle.into());
        inputs.insert(ent, Default::default());
    }

    let ground_pos = Vector2::new(0.0, -10.0);
    collision_world.create_rectangle_shape(ground_pos, 50.0, 2.0);
}

#[derive(Clone, Debug, HasSchema, Default)]
#[repr(C)]
struct Input {
    up: bool,
    right: bool,
    down: bool,
    left: bool,
}

#[rustfmt::skip]
impl Input {
    pub fn up() -> Self { Self { up: true, ..Default::default() }}
    pub fn right() -> Self { Self { right: true, ..Default::default() }}
    pub fn down() -> Self { Self { down: true, ..Default::default() }}
    pub fn left() -> Self { Self { left: true, ..Default::default() }}
    pub fn none() -> Self { Default::default() }

    pub fn to_vec2(&self) -> Vec2 {
        let x = (self.right as i8 - self.left as i8) as f32;
        let y = (self.up as i8 - self.down as i8) as f32;
        Vec2 { x, y }
    }
}

fn read_inputs(entities: Res<Entities>, mut inputs: CompMut<Input>) {
    for (index, (_, input)) in entities.iter_with(&mut inputs).enumerate() {
        *input = match index % 5 {
            0 => Input::up(),
            1 => Input::down(),
            2 => Input::left(),
            3 => Input::right(),
            _ => Input::none(),
        };
    }
}

const INPUT_MULTIPLIER: f32 = 0.70;

fn apply_inputs(
    entities: Res<Entities>,
    input: Comp<Input>,
    handle: Comp<PhysicsHandle>,
    mut collision_world: CollisionWorld,
) {
    for (entity, (input, handle)) in entities.iter_with((&input, &handle)) {
        let vel: Vector2<f32> = input.to_vec2().into();
        //collision_world.apply_vel((*handle).into(), vel * INPUT_MULTIPLIER);
    }
}

/// Step physics engine
fn step_physics_world(mut collision_world: CollisionWorld) {
    let time = Instant::now();
    collision_world.step();
    println!("Physics step time: {:?}", time.elapsed());
}

fn update_physics_positions(
    entities: Res<Entities>,
    mut pos: CompMut<Pos>,
    mut vel: CompMut<Vel>,
    handle: Comp<PhysicsHandle>,
    mut collision_world: CollisionWorld,
) {
    let time = Instant::now();
    for (_, (pos, vel, handle)) in entities.iter_with((&mut pos, &mut vel, &handle)) {
        let body = collision_world.get_body((*handle).into()).unwrap();
        *pos = (*body.translation()).into();
        *vel = (*body.linvel()).into();
    }
    println!("Elapsed: {:?}", time.elapsed())
}

// Alternative way to update positions, seems to be a multi-fold faster
fn update_physics_positions2(
    mut pos: CompMut<Pos>,
    mut vel: CompMut<Vel>,
    collision_world: CollisionWorld,
) {
    let time = Instant::now();
    for body in collision_world.body_iter() {
        let ent: Entity = RapierUserData::entity(body.user_data);
        if let Some(pos) = pos.get_mut(ent) {
            *pos = (*body.translation()).into();
        }
        if let Some(vel) = vel.get_mut(ent) {
            *vel = (*body.linvel()).into();
        }
    }
    println!("Elapsed2: {:?}", time.elapsed())
}

//
//Boilerplate stuff
//

impl AsRef<Vec2> for Pos {
    fn as_ref(&self) -> &Vec2 {
        &self.0
    }
}
impl AsMut<Vec2> for Pos {
    fn as_mut(&mut self) -> &mut Vec2 {
        &mut self.0
    }
}
impl From<Vec2> for Pos {
    fn from(value: Vec2) -> Self {
        Self(value)
    }
}
impl From<nalgebra::Vector2<f32>> for Pos {
    fn from(v: nalgebra::Vector2<f32>) -> Self {
        Pos(v.into())
    }
}

impl AsRef<Vec2> for Vel {
    fn as_ref(&self) -> &Vec2 {
        &self.0
    }
}
impl AsMut<Vec2> for Vel {
    fn as_mut(&mut self) -> &mut Vec2 {
        &mut self.0
    }
}
impl From<Vec2> for Vel {
    fn from(value: Vec2) -> Self {
        Self(value)
    }
}
impl From<nalgebra::Vector2<f32>> for Vel {
    fn from(v: nalgebra::Vector2<f32>) -> Self {
        Vel(v.into())
    }
}

use bones_ecs::prelude::*;
use rapier2d::prelude::*;
//use nalgebra::Vector2;
//use nalgebra::*;

use nalgebra::{Matrix2x1, Vector2};

#[derive(Clone, Debug, HasSchema, Default)]
#[repr(C)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}
impl From<(f32, f32)> for Vec2 {
    fn from(tuple: (f32, f32)) -> Self {
        Vec2 {
            x: tuple.0,
            y: tuple.1,
        }
    }
}
impl From<Vec2> for (f32, f32) {
    fn from(vec2: Vec2) -> Self {
        (vec2.x, vec2.y)
    }
}
// Implement From<Vector2<f32>> for Vec2
impl From<Vector2<f32>> for Vec2 {
    fn from(value: Vector2<f32>) -> Self {
        Self {
            x: value[0],
            y: value[1],
        }
    }
}

// Implement From<Vec2> for Vector2<f32>
impl From<Vec2> for Vector2<f32> {
    fn from(value: Vec2) -> Self {
        Vector2::new(value.x, value.y)
    }
}

#[derive(HasSchema)]
pub struct RapierContext {
    pub gravity: Vector2<f32>,
    pub rigid_bodies: RigidBodySet,
    pub colliders: ColliderSet,
    pub integration_parameters: IntegrationParameters,
    pub physics_pipeline: PhysicsPipeline,
    pub island_manager: IslandManager,
    pub broad_phase: DefaultBroadPhase,
    pub narrow_phase: NarrowPhase,
    pub impulse_joint_set: ImpulseJointSet,
    pub multibody_joint_set: MultibodyJointSet,
    pub ccd_solver: CCDSolver,
    pub query_pipeline: QueryPipeline,
}

impl Clone for RapierContext {
    fn clone(&self) -> Self {
        Self {
            gravity: self.gravity,
            rigid_bodies: self.rigid_bodies.clone(),
            colliders: self.colliders.clone(),
            integration_parameters: self.integration_parameters,
            physics_pipeline: Default::default(),
            island_manager: self.island_manager.clone(),
            broad_phase: self.broad_phase.clone(),
            narrow_phase: self.narrow_phase.clone(),
            impulse_joint_set: self.impulse_joint_set.clone(),
            multibody_joint_set: self.multibody_joint_set.clone(),
            ccd_solver: self.ccd_solver.clone(),
            query_pipeline: self.query_pipeline.clone(),
        }
    }
}

impl Default for RapierContext {
    fn default() -> Self {
        Self {
            gravity: Vector::new(0.0, -9.81),
            rigid_bodies: Default::default(),
            colliders: Default::default(),
            integration_parameters: Default::default(),
            physics_pipeline: Default::default(),
            island_manager: Default::default(),
            broad_phase: Default::default(),
            narrow_phase: Default::default(),
            impulse_joint_set: Default::default(),
            multibody_joint_set: Default::default(),
            ccd_solver: Default::default(),
            query_pipeline: Default::default(),
        }
    }
}

impl RapierContext {
    pub fn new(gravity: Vector2<f32>) -> Self {
        Self {
            gravity,
            ..Default::default()
        }
    }

    pub fn debug_render(&self, backend: &mut impl DebugRenderBackend) {
        let mut pipeline = DebugRenderPipeline::default();
        pipeline.render_colliders(backend, &self.rigid_bodies, &self.colliders);
    }

    pub fn insert_collider_with_parent(
        &mut self,
        coll: impl Into<Collider>,
        body_handle: RigidBodyHandle,
    ) -> ColliderHandle {
        self.colliders
            .insert_with_parent(coll, body_handle, &mut self.rigid_bodies)
    }

    pub fn step(&mut self) {
        self.physics_pipeline.step(
            &self.gravity,
            &self.integration_parameters,
            &mut self.island_manager,
            &mut self.broad_phase,
            &mut self.narrow_phase,
            &mut self.rigid_bodies,
            &mut self.colliders,
            &mut self.impulse_joint_set,
            &mut self.multibody_joint_set,
            &mut self.ccd_solver,
            Some(&mut self.query_pipeline),
            &(),
            &(),
        );
    }
}

#[derive(Copy, Clone, Debug, HasSchema, Default)]
#[repr(C)]
pub struct PhysicsHandle(u32, u32);

impl From<PhysicsHandle> for RigidBodyHandle {
    fn from(value: PhysicsHandle) -> Self {
        Self::from_raw_parts(value.0, value.1)
    }
}

impl From<RigidBodyHandle> for PhysicsHandle {
    fn from(value: RigidBodyHandle) -> Self {
        let (a, b) = value.into_raw_parts();
        Self(a, b)
    }
}

#[derive(HasSchema, Default, Clone)]
pub struct Actor;

#[derive(SystemParam)]
pub struct CollisionWorld<'a> {
    ctx: ResMutInit<'a, RapierContext>,
    //SystemParam derive macro does not seem to work, if there is only 1 field
    actors: ResInit<'a, Actor>,
}

impl<'a> CollisionWorld<'a> {
    pub fn step(&mut self) {
        self.ctx.step();
    }

    pub fn body_iter(&self) -> impl Iterator<Item = &RigidBody> {
        self.ctx.rigid_bodies.iter().map(|(_, body)| body)
    }

    pub fn body_iter_mut(&mut self) -> impl Iterator<Item = &mut RigidBody> {
        self.ctx.rigid_bodies.iter_mut().map(|(_, body)| body)
    }

    pub fn get_body(&mut self, handle: RigidBodyHandle) -> Option<&RigidBody> {
        self.ctx.rigid_bodies.get(handle)
    }

    pub fn get_body_mut(&mut self, handle: RigidBodyHandle) -> Option<&mut RigidBody> {
        self.ctx.rigid_bodies.get_mut(handle)
    }

    pub fn apply_vel(&mut self, handle: RigidBodyHandle, force: Vector2<f32>) {
        let Some(body) = self.ctx.rigid_bodies.get_mut(handle) else {
            return;
        };
        let vel = body.linvel() + force;
        body.set_linvel(vel, true);
    }

    pub fn get_pos(&mut self, handle: RigidBodyHandle) -> impl Into<Vec2> {
        *self.ctx.rigid_bodies.get(handle).unwrap().translation()
    }

    pub fn create_dynamic_rigid_body(
        &mut self,
        entity: Entity,
        collider_builder: ColliderBuilder,
        pos: Vector2<f32>,
        rot: f32,
    ) -> RigidBodyHandle {
        let userdata = RapierUserData::from(entity);
        let body = RigidBodyBuilder::dynamic()
            .translation(pos)
            .rotation(rot)
            .user_data(userdata)
            .build();
        let body_handle = self.ctx.rigid_bodies.insert(body);
        self.ctx
            .insert_collider_with_parent(collider_builder, body_handle);
        body_handle
    }

    pub fn insert_collider(&mut self, collider_builder: ColliderBuilder) {
        self.ctx.colliders.insert(collider_builder);
    }

    pub fn create_ball(
        &mut self,
        entity: Entity,
        radius: f32,
        mass: f32,
        pos: Vector2<f32>,
    ) -> RigidBodyHandle {
        let collider_builder = ColliderBuilder::ball(radius).restitution(0.5).mass(mass);
        self.create_dynamic_rigid_body(entity, collider_builder, pos, Default::default())
    }

    pub fn create_rectangle(
        &mut self,
        entity: Entity,
        pos: Vector2<f32>,
        rot: f32,
        width: f32,
        height: f32,
    ) -> RigidBodyHandle {
        let collider_builder = ColliderBuilder::cuboid(width / 2.0, height / 2.0).restitution(1.0);
        self.create_dynamic_rigid_body(entity, collider_builder, pos, rot)
    }

    pub fn create_rectangle_shape(&mut self, pos: Vector2<f32>, width: f32, height: f32) {
        let collider_builder = ColliderBuilder::cuboid(width / 2.0, height / 2.0)
            .position(pos.into())
            .restitution(1.0);

        self.ctx.colliders.insert(collider_builder);
    }
}

pub struct RapierUserData;
impl RapierUserData {
    pub fn from(e: Entity) -> u128 {
        let mut out = 0u128;

        out |= e.index() as u128;
        out |= (e.generation() as u128) << 32;

        out
    }

    pub fn entity(user_data: u128) -> Entity {
        let index = (u32::MAX as u128) & user_data;
        let generation = (u32::MAX as u128) & (user_data >> 32);
        Entity::new(index as u32, generation as u32)
    }
}

use std::{borrow::Borrow, iter::Map, time::Instant};

use bones_ecs::prelude::*;
use miniquad::Context;
use nona::{Canvas, Color, Transform};
use nonaquad::nvgimpl::{self, RendererCtx};
use rapier2d::prelude::*;

use crate::physics::RapierContext;

pub struct Game {
    world: World,
    stages: SystemStages,
}

impl Game {
    pub fn new(world: World, stages: SystemStages) -> Self {
        Game { world, stages }
    }
}

pub struct DebugRenderer {
    game: Game,
    renderer: nvgimpl::Renderer,
    nona: nona::Context,
}

impl DebugRenderer {
    fn new(game: Game, ctx: &mut Context) -> Self {
        let mut renderer = nvgimpl::Renderer::create(ctx).unwrap();
        let nona = nona::Context::create(&mut renderer.with_context(ctx)).unwrap();

        DebugRenderer {
            game,
            renderer,
            nona,
        }
    }

    pub fn start(game: Game) {
        miniquad::start(
            miniquad::conf::Conf {
                high_dpi: true,
                window_title: String::from("Game with Debug Renderer"),
                ..Default::default()
            },
            |ctx| Box::new(Self::new(game, ctx)),
        );
    }
}

impl miniquad::EventHandler for DebugRenderer {
    fn update(&mut self, _ctx: &mut Context) {
        let time = Instant::now();
        self.game.stages.run(&mut self.game.world);
        println!("Frame update time: {:?}", time.elapsed());
    }

    fn draw(&mut self, ctx: &mut Context) {
        let rapier = self.game.world.resource::<RapierContext>();
        let (screen_width, screen_height) = ctx.screen_size();
        let tx = screen_width / 2.0;
        let ty = screen_height / 2.0;

        let canvas_translate = Transform::translate(tx, ty);
        let canvas_scale = Transform::scale(50.0, -50.0);

        let draw_duration = Instant::now();

        //ctx.

        self.nona
            .attach_renderer(&mut self.renderer.with_context(ctx), |canvas| {
                canvas
                    .begin_frame(Some(Color::rgb_i(128, 128, 255)))
                    .unwrap();

                canvas.transform(canvas_translate);
                canvas.transform(canvas_scale);

                let time = Instant::now();
                let mut renderer = NonaRapierDebugRenderer::new(canvas);
                rapier.debug_render(&mut renderer);
                println!("Debug render duration: {:?}", time.elapsed());
                canvas.end_frame().unwrap();
            });
        ctx.commit_frame();
        println!("Canvas draw duration: {:?}", draw_duration.elapsed());
    }
}

fn draw_rectangle(
    canvas: &mut nona::Canvas<RendererCtx>,
    position: &Isometry<Real>,
    half_extents: Vector<Real>,
    color: Color,
) {
    let rotation = position.rotation;

    // Calculate the four corners of the rectangle
    let corners = [
        Vector::new(-half_extents.x, -half_extents.y),
        Vector::new(half_extents.x, -half_extents.y),
        Vector::new(half_extents.x, half_extents.y),
        Vector::new(-half_extents.x, half_extents.y),
    ];

    // Rotate and translate the corners
    let transformed_corners: Vec<_> = corners
        .iter()
        .map(|corner| {
            let rotated = rotation * corner;
            let translated = rotated + position.translation.vector;
            nona::Point::new(translated.x as f32, translated.y as f32)
        })
        .collect();

    // Draw the rectangle
    canvas.begin_path();
    canvas.move_to(transformed_corners[0]);
    for corner in &transformed_corners[1..] {
        canvas.line_to(*corner);
    }
    canvas.close_path();

    canvas.fill_paint(nona::Paint::from(color));
    canvas.fill().unwrap();

    /*
    // Optionally, draw an outline
    canvas.stroke_paint(nona::Paint::from(Color::rgb_i(0, 0, 0))); // Black outline
    canvas.stroke().unwrap();
    */
}

struct NonaRapierDebugRenderer<'a, 'b, 'c>
where
    'b: 'a,
{
    drawn_colliders: HashSet<ColliderHandle>,
    canvas: &'a mut nona::Canvas<'c, RendererCtx<'b>>,
}

impl<'a, 'b, 'c> NonaRapierDebugRenderer<'a, 'b, 'c> {
    pub fn new(canvas: &'a mut nona::Canvas<'c, RendererCtx<'b>>) -> Self {
        Self {
            canvas,
            drawn_colliders: Default::default(),
        }
    }
}

impl<'a, 'b, 'c> DebugRenderBackend for NonaRapierDebugRenderer<'a, 'b, 'c>
where
    'b: 'a,
{
    fn draw_line(
        &mut self,
        _object: rapier2d::prelude::DebugRenderObject,
        a: rapier2d::prelude::Point<f32>,
        b: rapier2d::prelude::Point<f32>,
        color: [f32; 4],
    ) {
        if let DebugRenderObject::Collider(handle, collider) = _object {
            if self.drawn_colliders.contains(&handle) {
                return;
            }

            // better drawing (than line drawing) for supported shapes
            if draw_shape(self.canvas, &handle, collider) {
                self.drawn_colliders.insert(handle);
                return;
            }
        }

        self.canvas.begin_path();
        self.canvas.move_to(to_nona_point(a.coords));
        self.canvas.line_to(to_nona_point(b.coords));

        self.canvas.stroke_paint(nona::Paint::from(Color::from((
            color[0], color[1], color[2], color[3],
        ))));
        self.canvas.stroke_width(0.03);
        self.canvas.stroke().unwrap();
    }
}

fn draw_shape(
    canvas: &mut nona::Canvas<RendererCtx>,
    handle: &ColliderHandle,
    collider: &Collider,
) -> bool {
    let color = get_color_by_index(handle.into_raw_parts().0);
    let position = collider.position();
    let translation = position.translation;

    match collider.shape().as_typed_shape() {
        TypedShape::Ball(ball) => {
            draw_circle(canvas, translation.vector, ball.radius, color);
        }
        TypedShape::Cuboid(cuboid) => {
            draw_rectangle(canvas, position, cuboid.half_extents, color);
        }
        TypedShape::Capsule(capsule) => draw_capsule(
            canvas,
            position,
            capsule.radius,
            capsule.half_height(),
            color,
        ),
        TypedShape::Segment(segment) => {
            draw_segment(canvas, position, segment.a.coords, segment.b.coords, color)
        }
        TypedShape::Triangle(triangle) => draw_triangle(
            canvas,
            position,
            triangle.a.coords,
            triangle.b.coords,
            triangle.c.coords,
            color,
        ),
        _ => {
            // Ignore other shapes
            return false;
        }
    }
    return true;
}

struct NonaDebugRenderer<'a, 'b: 'a> {
    canvas: &'a mut nona::Canvas<'b, RendererCtx<'b>>,
}

impl<'a, 'b: 'a> DebugRenderBackend for NonaDebugRenderer<'a, 'b> {
    fn draw_line(
        &mut self,
        object: rapier2d::prelude::DebugRenderObject,
        a: rapier2d::prelude::Point<f32>,
        b: rapier2d::prelude::Point<f32>,
        color: [f32; 4],
    ) {
        self.canvas.begin_path();
        self.canvas.move_to(to_nona_point(a.coords));
        self.canvas.line_to(to_nona_point(b.coords));

        self.canvas.stroke_paint(nona::Paint::from(Color::from((
            color[0], color[1], color[2], color[3],
        ))));
        self.canvas.stroke().unwrap();
    }
    // Implement other methods of DebugRenderBackend as needed
}

struct CanvasDebugRenderer<'a>(&'a mut nona::Canvas<'a, RendererCtx<'a>>);

impl<'a> DebugRenderBackend for CanvasDebugRenderer<'a> {
    fn draw_line(
        &mut self,
        _object: rapier2d::prelude::DebugRenderObject,
        a: rapier2d::prelude::Point<f32>,
        b: rapier2d::prelude::Point<f32>,
        color: [f32; 4],
    ) {
        self.0.begin_path();
        self.0.move_to(to_nona_point(a.coords));
        self.0.line_to(to_nona_point(b.coords));

        self.0.stroke_paint(nona::Paint::from(Color::from((
            color[0], color[1], color[2], color[3],
        ))));
        self.0.stroke().unwrap();
    }
}

fn draw_circle(
    canvas: &mut nona::Canvas<RendererCtx>,
    translation: Vector<Real>,
    radius: Real,
    color: Color,
) {
    canvas.begin_path();
    canvas.circle(nona::Point::new(translation.x, translation.y), radius);
    canvas.fill_paint(nona::Paint::from(color));
    canvas.fill().unwrap();
}

fn draw_capsule(
    canvas: &mut nona::Canvas<RendererCtx>,
    position: &Isometry<Real>,
    radius: Real,
    half_height: Real,
    color: Color,
) {
    let direction = position.rotation * Vector::y();
    let start = to_nona_point(position.translation.vector + direction * half_height);
    let end = to_nona_point(position.translation.vector - direction * half_height);

    // Draw the rectangular part
    canvas.begin_path();
    canvas.move_to(nona::Point::new((start.x - radius) as f32, start.y as f32));
    canvas.line_to(nona::Point::new((end.x - radius) as f32, end.y as f32));
    canvas.line_to(nona::Point::new((end.x + radius) as f32, end.y as f32));
    canvas.line_to(nona::Point::new((start.x + radius) as f32, start.y as f32));
    canvas.close_path();

    canvas.fill_paint(nona::Paint::from(color));
    canvas.fill().unwrap();

    // Draw the end caps
    canvas.circle(nona::Point::new(start.x as f32, start.y as f32), radius);
    canvas.fill().unwrap();
    canvas.circle(nona::Point::new(end.x as f32, end.y as f32), radius);
    canvas.fill().unwrap();
}

fn draw_segment(
    canvas: &mut nona::Canvas<RendererCtx>,
    position: &Isometry<Real>,
    a: Vector<Real>,
    b: Vector<Real>,
    color: Color,
) {
    let start = to_nona_point(position * a);
    let end = to_nona_point(position * b);

    canvas.begin_path();
    canvas.move_to(start);
    canvas.line_to(end);

    canvas.stroke_paint(nona::Paint::from(color));
    canvas.stroke().unwrap();
}

fn draw_triangle(
    canvas: &mut nona::Canvas<RendererCtx>,
    position: &Isometry<Real>,
    a: Vector<Real>,
    b: Vector<Real>,
    c: Vector<Real>,
    color: Color,
) {
    let points = [a, b, c].map(|p| to_nona_point(position * p));

    canvas.begin_path();
    canvas.move_to(points[0]);
    canvas.line_to(points[1]);
    canvas.line_to(points[2]);
    canvas.close_path();

    canvas.fill_paint(nona::Paint::from(color));
    canvas.fill().unwrap();
    canvas.stroke_paint(nona::Paint::from(Color::rgb_i(0, 0, 0)));
    canvas.stroke().unwrap();
}

fn to_nona_point(vec: Vector<Real>) -> nona::Point {
    nona::Point::new(vec.x, vec.y)
}

pub fn get_color_by_index(index: u32) -> Color {
    // Use golden ratio to spread hues evenly
    let golden_ratio_conjugate = 0.618_034;
    let hue = (index as f32 * golden_ratio_conjugate) % 1.0;

    // Convert HSL to RGB
    let saturation = 0.5;
    let lightness = 0.6;

    hsl_to_rgb(hue, saturation, lightness)
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> Color {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h * 6.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;

    let (r, g, b) = match (h * 6.0).floor() as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };

    Color::rgb_i(
        ((r + m) * 255.0) as u8,
        ((g + m) * 255.0) as u8,
        ((b + m) * 255.0) as u8,
    )
}

use bones_ecs::prelude::*;
use macroquad::prelude::*;
use rapier2d::prelude::*;
use std::time::Instant;

use crate::physics::RapierContext;

pub struct Game {
    world: World,
    stages: SystemStages,
}

impl Game {
    pub fn new(world: World, stages: SystemStages) -> Self {
        Game { world, stages }
    }

    fn update(&mut self) {
        let time = Instant::now();
        self.stages.run(&mut self.world);
        println!("Frame update time: {:?}", time.elapsed());
    }

    fn draw(&mut self) {
        let rapier = self.world.resource::<RapierContext>();
        let draw_duration = Instant::now();
        let mut renderer = RapierDebugRenderer::new(18.0, 0.0, 0.0);
        rapier.debug_render(&mut renderer);
        println!("Debug render duration: {:?}", draw_duration.elapsed());
    }
}

pub fn start(game: Game) {
    let conf = Conf {
        window_title: "Debug rendering".to_owned(),
        window_width: 800,
        window_height: 600,
        ..Default::default()
    };

    macroquad::Window::from_config(conf, run(game));
}

async fn run(mut game: Game) {
    loop {
        game.update();
        game.draw();

        next_frame().await;
    }
}

struct RapierDebugRenderer {
    zoom: f32,
    camera_x: f32,
    camera_y: f32,
}

impl RapierDebugRenderer {
    pub fn new(zoom: f32, camera_x: f32, camera_y: f32) -> Self {
        Self {
            zoom,
            camera_x,
            camera_y,
        }
    }

    fn world_to_screen(&self, x: f32, y: f32) -> (f32, f32) {
        let screen_x = (x - self.camera_x) * self.zoom + screen_width() / 2.0;
        let screen_y = (self.camera_y - y) * self.zoom + screen_height() / 2.0;
        (screen_x, screen_y)
    }

    fn world_draw_line(
        &self,
        a: rapier2d::prelude::Point<f32>,
        b: rapier2d::prelude::Point<f32>,
        color: Color,
    ) {
        let (x1, y1) = self.world_to_screen(a.x, a.y);
        let (x2, y2) = self.world_to_screen(b.x, b.y);
        draw_line(x1, y1, x2, y2, 1.0, color);
    }
}

impl DebugRenderBackend for RapierDebugRenderer {
    fn draw_line(
        &mut self,
        _object: rapier2d::prelude::DebugRenderObject,
        a: rapier2d::prelude::Point<f32>,
        b: rapier2d::prelude::Point<f32>,
        color: [f32; 4],
    ) {
        self.world_draw_line(a, b, Color::new(color[0], color[1], color[2], color[3]));
    }
}

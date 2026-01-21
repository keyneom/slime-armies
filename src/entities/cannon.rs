use crate::math::Vec2;
use crate::world::ChunkManager;
use rand::Rng;

const ACCEL: f32 = 0.02;
const SPEED_MAX: f32 = 0.7;

#[derive(Debug, Clone)]
pub struct Cannon {
    pub id: usize,
    pub alive: bool,
    pub pos: Vec2,
    pub speed: Vec2,
    pub dir: Vec2,
    pub look_dir: Vec2,
    pub target_offset: Vec2,
    shoot_timer: u32,
}

impl Cannon {
    pub fn new<R: Rng>(id: usize, screen_width: f32, screen_height: f32, rng: &mut R) -> Self {
        let x = rng.gen::<f32>() * 100.0 - 50.0;
        let y = rng.gen::<f32>() * 100.0 - 50.0;

        let pos = Vec2::new(
            if x < 0.0 { -10.0 + x } else { screen_width + 10.0 + x },
            if y < 0.0 { -10.0 + y } else { screen_height + 10.0 + y },
        );

        Self {
            id,
            alive: true,
            pos,
            speed: Vec2::ZERO,
            dir: Vec2::ZERO,
            look_dir: Vec2::ZERO,
            target_offset: Vec2::new(
                rng.gen::<f32>() * 1.2 - 0.6,
                rng.gen::<f32>() * 1.2 - 0.6,
            ),
            shoot_timer: (id * 10) as u32,
        }
    }

    /// Spawn cannon at a distance from player position (for infinite world)
    pub fn new_around<R: Rng>(id: usize, player_pos: Vec2, spawn_distance: f32, rng: &mut R) -> Self {
        // Random angle around the player
        let angle = rng.gen::<f32>() * std::f32::consts::TAU;
        let distance = spawn_distance + rng.gen::<f32>() * 100.0;

        let pos = Vec2::new(
            player_pos.x + angle.cos() * distance,
            player_pos.y + angle.sin() * distance,
        );

        Self {
            id,
            alive: true,
            pos,
            speed: Vec2::ZERO,
            dir: Vec2::ZERO,
            look_dir: Vec2::ZERO,
            target_offset: Vec2::new(
                rng.gen::<f32>() * 1.2 - 0.6,
                rng.gen::<f32>() * 1.2 - 0.6,
            ),
            shoot_timer: (id * 10) as u32,
        }
    }

    /// Spawn cannon at a distance from player with obstacle checking
    pub fn new_around_validated<R: Rng>(
        id: usize,
        player_pos: Vec2,
        spawn_distance: f32,
        chunks: &ChunkManager,
        rng: &mut R,
    ) -> Self {
        // Try up to 10 positions to avoid obstacles
        let mut pos = Vec2::ZERO;
        for _ in 0..10 {
            let angle = rng.gen::<f32>() * std::f32::consts::TAU;
            let distance = spawn_distance + rng.gen::<f32>() * 100.0;

            pos = Vec2::new(
                player_pos.x + angle.cos() * distance,
                player_pos.y + angle.sin() * distance,
            );

            // Check if position is clear of obstacles and player
            if !chunks.collides_with_obstacle(pos, 10.0) && pos.distance(player_pos) > 50.0 {
                break;
            }
        }

        Self {
            id,
            alive: true,
            pos,
            speed: Vec2::ZERO,
            dir: Vec2::ZERO,
            look_dir: Vec2::ZERO,
            target_offset: Vec2::new(
                rng.gen::<f32>() * 1.2 - 0.6,
                rng.gen::<f32>() * 1.2 - 0.6,
            ),
            shoot_timer: (id * 10) as u32,
        }
    }

    pub fn new_at_position<R: Rng>(id: usize, pos: Vec2, rng: &mut R) -> Self {
        Self {
            id,
            alive: true,
            pos,
            speed: Vec2::ZERO,
            dir: Vec2::ZERO,
            look_dir: Vec2::ZERO,
            target_offset: Vec2::new(
                rng.gen::<f32>() * 1.2 - 0.6,
                rng.gen::<f32>() * 1.2 - 0.6,
            ),
            shoot_timer: (id * 10) as u32,
        }
    }

    pub fn update(&mut self, player_pos: Vec2, frame_count: u32, screen_width: f32, screen_height: f32) -> Option<CannonEvent> {
        if !self.alive {
            return None;
        }

        // Movement towards player with offset
        let target = player_pos - self.pos;
        let distance = target.length();

        let mut move_target = Vec2::new(
            target.x + self.target_offset.x * distance,
            target.y + self.target_offset.y * distance,
        );
        move_target.normalize_mut();

        self.speed.x += move_target.x * ACCEL;
        self.speed.y += move_target.y * ACCEL;

        if self.speed.x != 0.0 || self.speed.y != 0.0 {
            self.dir = self.speed.normalize();
            if self.speed.length() > SPEED_MAX {
                self.speed = self.dir * SPEED_MAX;
            }
        }

        self.pos += self.speed;

        // Aiming
        self.look_dir = target.normalize();

        // Shooting (only when on screen)
        let on_screen = self.pos.x > 0.0 && self.pos.x < screen_width
            && self.pos.y > 0.0 && self.pos.y < screen_height;

        if on_screen && (frame_count + self.shoot_timer) % 100 == 0 {
            let projectile_pos = self.pos + self.look_dir * 4.0;
            let projectile_speed = self.look_dir * 2.0;
            return Some(CannonEvent::Shoot { pos: projectile_pos, speed: projectile_speed });
        }

        None
    }

    /// Update for infinite world - takes whether cannon is on screen and chunks for collision
    pub fn update_infinite(&mut self, player_pos: Vec2, frame_count: u32, on_screen: bool, chunks: &ChunkManager) -> Option<CannonEvent> {
        if !self.alive {
            return None;
        }

        // Movement towards player with offset
        let target = player_pos - self.pos;
        let distance = target.length();

        let mut move_target = Vec2::new(
            target.x + self.target_offset.x * distance,
            target.y + self.target_offset.y * distance,
        );
        move_target.normalize_mut();

        self.speed.x += move_target.x * ACCEL;
        self.speed.y += move_target.y * ACCEL;

        if self.speed.x != 0.0 || self.speed.y != 0.0 {
            self.dir = self.speed.normalize();
            if self.speed.length() > SPEED_MAX {
                self.speed = self.dir * SPEED_MAX;
            }
        }

        let new_pos = self.pos + self.speed;

        // Check obstacle collision
        if !chunks.collides_with_obstacle(new_pos, self.radius()) {
            self.pos = new_pos;
        } else {
            // Try sliding along obstacles
            let new_x = Vec2::new(new_pos.x, self.pos.y);
            let new_y = Vec2::new(self.pos.x, new_pos.y);

            if !chunks.collides_with_obstacle(new_x, self.radius()) {
                self.pos.x = new_pos.x;
            } else {
                self.speed.x *= -0.5;
            }
            if !chunks.collides_with_obstacle(new_y, self.radius()) {
                self.pos.y = new_pos.y;
            } else {
                self.speed.y *= -0.5;
            }
        }

        // Aiming
        self.look_dir = target.normalize();

        // Shooting (only when on screen)
        if on_screen && (frame_count + self.shoot_timer) % 100 == 0 {
            let projectile_pos = self.pos + self.look_dir * 4.0;
            let projectile_speed = self.look_dir * 2.0;
            return Some(CannonEvent::Shoot { pos: projectile_pos, speed: projectile_speed });
        }

        None
    }

    pub fn kill(&mut self) {
        self.alive = false;
    }

    pub fn radius(&self) -> f32 {
        7.0
    }

    pub fn bump(&mut self, dir: Vec2, amount: f32) {
        self.pos += dir * amount;
        self.speed.reflect_mut(dir);
    }
}

#[derive(Debug)]
pub enum CannonEvent {
    Shoot { pos: Vec2, speed: Vec2 },
}

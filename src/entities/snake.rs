use crate::math::Vec2;
use crate::world::ChunkManager;
use rand::Rng;

const SPEED_MAX: f32 = 3.0;

#[derive(Debug, Clone)]
pub struct Snake {
    pub id: usize,
    pub alive: bool,
    pub pos: Vec2,
    pub speed: Vec2,
    pub dir: Vec2,
    pub size: f32,
}

impl Snake {
    pub fn new<R: Rng>(id: usize, previous: Option<&Snake>, screen_width: f32, screen_height: f32, rng: &mut R) -> Self {
        let pos = if let Some(prev) = previous {
            prev.pos
        } else {
            let x = rng.gen::<f32>() * 100.0 - 50.0;
            let y = rng.gen::<f32>() * 100.0 - 50.0;
            Vec2::new(
                if x < 0.0 { -50.0 + x } else { screen_width + 50.0 + x },
                if y < 0.0 { -50.0 + y } else { screen_height + 50.0 + y },
            )
        };

        // Head is bigger, tail segments get smaller
        let size = (17.0 - id as f32).max(9.0);

        Self {
            id,
            alive: true,
            pos,
            speed: Vec2::ZERO,
            dir: Vec2::ZERO,
            size,
        }
    }

    /// Spawn snake segment at a distance from player position (for infinite world)
    pub fn new_around<R: Rng>(id: usize, previous: Option<&Snake>, player_pos: Vec2, spawn_distance: f32, rng: &mut R) -> Self {
        let pos = if let Some(prev) = previous {
            prev.pos
        } else {
            // Random angle around the player for the head
            let angle = rng.gen::<f32>() * std::f32::consts::TAU;
            let distance = spawn_distance + rng.gen::<f32>() * 100.0;

            Vec2::new(
                player_pos.x + angle.cos() * distance,
                player_pos.y + angle.sin() * distance,
            )
        };

        // Head is bigger, tail segments get smaller
        let size = (17.0 - id as f32).max(9.0);

        Self {
            id,
            alive: true,
            pos,
            speed: Vec2::ZERO,
            dir: Vec2::ZERO,
            size,
        }
    }

    /// Spawn snake segment at a distance from player with obstacle checking
    pub fn new_around_validated<R: Rng>(
        id: usize,
        previous: Option<&Snake>,
        player_pos: Vec2,
        spawn_distance: f32,
        chunks: &ChunkManager,
        rng: &mut R,
    ) -> Self {
        let pos = if let Some(prev) = previous {
            prev.pos
        } else {
            // Try up to 10 positions to avoid obstacles (for head only)
            let mut pos = Vec2::ZERO;
            for _ in 0..10 {
                let angle = rng.gen::<f32>() * std::f32::consts::TAU;
                let distance = spawn_distance + rng.gen::<f32>() * 100.0;

                pos = Vec2::new(
                    player_pos.x + angle.cos() * distance,
                    player_pos.y + angle.sin() * distance,
                );

                // Check if position is clear of obstacles and player
                if !chunks.collides_with_obstacle(pos, 12.0) && pos.distance(player_pos) > 50.0 {
                    break;
                }
            }
            pos
        };

        // Head is bigger, tail segments get smaller
        let size = (17.0 - id as f32).max(9.0);

        Self {
            id,
            alive: true,
            pos,
            speed: Vec2::ZERO,
            dir: Vec2::ZERO,
            size,
        }
    }

    pub fn new_at_position(id: usize, previous: Option<&Snake>, pos: Vec2) -> Self {
        let spawn_pos = if let Some(prev) = previous { prev.pos } else { pos };
        let size = (17.0 - id as f32).max(9.0);

        Self {
            id,
            alive: true,
            pos: spawn_pos,
            speed: Vec2::ZERO,
            dir: Vec2::new(0.0, -1.0),
            size,
        }
    }

    pub fn update(&mut self, player_pos: Vec2, previous: Option<&Snake>) {
        if !self.alive {
            return;
        }

        // Calculate movement target
        let (move_target, accel) = if let Some(prev) = previous {
            if prev.alive {
                let target = prev.pos - self.pos;
                let dist = target.length();
                (target.normalize(), dist * dist * 0.025)
            } else {
                let target = player_pos - self.pos;
                let distance = target.length();
                (target.normalize(), (100.0 - distance).max(0.05) * 0.003)
            }
        } else {
            // Head follows player
            let target = player_pos - self.pos;
            let distance = target.length();
            (target.normalize(), (100.0 - distance).max(0.05) * 0.003)
        };

        self.speed.x += move_target.x * accel;
        self.speed.y += move_target.y * accel;

        if self.speed.x != 0.0 || self.speed.y != 0.0 {
            self.dir = self.speed.normalize();
            if self.speed.length() > SPEED_MAX {
                self.speed = self.dir * SPEED_MAX;
            }
        }

        self.pos += self.speed;
    }

    pub fn get_closest_distance(snakes: &[Snake], player_pos: Vec2) -> Option<f32> {
        snakes
            .iter()
            .filter(|s| s.alive)
            .map(|s| s.pos.distance(player_pos))
            .min_by(|a, b| a.partial_cmp(b).unwrap())
    }

    pub fn kill(&mut self) {
        self.alive = false;
    }

    pub fn radius(&self) -> f32 {
        self.size / 2.0
    }

    pub fn bump(&mut self, dir: Vec2, amount: f32) {
        self.pos += dir * amount;
        self.speed.reflect_mut(dir);
    }
}

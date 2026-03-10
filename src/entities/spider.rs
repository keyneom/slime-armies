use crate::math::Vec2;
use crate::world::ChunkManager;
use rand::Rng;

const ACCEL: f32 = 0.08;
const SPEED_MAX: f32 = 2.5;

#[derive(Debug, Clone)]
pub struct Spider {
    pub id: usize,
    pub alive: bool,
    pub pos: Vec2,
    pub speed: Vec2,
    pub dir: Vec2,
    pub target_offset: Vec2,
}

impl Spider {
    pub fn new<R: Rng>(id: usize, screen_width: f32, screen_height: f32, rng: &mut R) -> Self {
        let x = rng.gen::<f32>() * 100.0 - 50.0;
        let y = rng.gen::<f32>() * 100.0 - 50.0;

        let pos = Vec2::new(
            if x < 0.0 {
                -50.0 + x
            } else {
                screen_width + 50.0 + x
            },
            if y < 0.0 {
                -50.0 + y
            } else {
                screen_height + 50.0 + y
            },
        );

        // Initialize dir toward screen center so legs are visible
        let center = Vec2::new(screen_width / 2.0, screen_height / 2.0);
        let dir = (center - pos).normalize();

        Self {
            id,
            alive: true,
            pos,
            speed: Vec2::ZERO,
            dir,
            target_offset: Vec2::new(rng.gen::<f32>() * 1.2 - 0.6, rng.gen::<f32>() * 1.2 - 0.6),
        }
    }

    /// Spawn spider at a distance from player position (for infinite world)
    /// Checks obstacle collision and finds valid spawn position
    pub fn new_around<R: Rng>(
        id: usize,
        player_pos: Vec2,
        spawn_distance: f32,
        rng: &mut R,
    ) -> Self {
        let pos = Self::find_valid_spawn(player_pos, spawn_distance, 6.0, rng);

        // Initialize dir toward player so legs are visible from spawn
        let dir = (player_pos - pos).normalize();

        Self {
            id,
            alive: true,
            pos,
            speed: Vec2::ZERO,
            dir,
            target_offset: Vec2::new(rng.gen::<f32>() * 1.2 - 0.6, rng.gen::<f32>() * 1.2 - 0.6),
        }
    }

    /// Find a valid spawn position that doesn't overlap with obstacles
    fn find_valid_spawn<R: Rng>(
        player_pos: Vec2,
        spawn_distance: f32,
        _radius: f32,
        rng: &mut R,
    ) -> Vec2 {
        // Try up to 10 times to find a valid position
        for _ in 0..10 {
            let angle = rng.gen::<f32>() * std::f32::consts::TAU;
            let distance = spawn_distance + rng.gen::<f32>() * 100.0;

            let pos = Vec2::new(
                player_pos.x + angle.cos() * distance,
                player_pos.y + angle.sin() * distance,
            );

            // Position is validated against obstacles in spawn_wave
            // Return the position - validation happens at a higher level
            return pos;
        }
        // Fallback - return the last attempted position
        let angle = rng.gen::<f32>() * std::f32::consts::TAU;
        let distance = spawn_distance + rng.gen::<f32>() * 100.0;
        Vec2::new(
            player_pos.x + angle.cos() * distance,
            player_pos.y + angle.sin() * distance,
        )
    }

    /// Spawn spider at a distance from player with obstacle checking
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
            if !chunks.collides_with_obstacle(pos, 8.0) && pos.distance(player_pos) > 50.0 {
                break;
            }
        }

        // Initialize dir toward player so legs are visible from spawn
        let dir = (player_pos - pos).normalize();

        Self {
            id,
            alive: true,
            pos,
            speed: Vec2::ZERO,
            dir,
            target_offset: Vec2::new(rng.gen::<f32>() * 1.2 - 0.6, rng.gen::<f32>() * 1.2 - 0.6),
        }
    }

    pub fn new_at_position<R: Rng>(id: usize, pos: Vec2, rng: &mut R) -> Self {
        Self {
            id,
            alive: true,
            pos,
            speed: Vec2::ZERO,
            dir: Vec2::new(0.0, -1.0),
            target_offset: Vec2::new(rng.gen::<f32>() * 1.2 - 0.6, rng.gen::<f32>() * 1.2 - 0.6),
        }
    }

    pub fn update(&mut self, player_pos: Vec2) -> Option<SpiderEvent> {
        self.update_with_chunks(player_pos, None)
    }

    /// Update for infinite world - checks obstacle collisions
    pub fn update_infinite(
        &mut self,
        player_pos: Vec2,
        chunks: &ChunkManager,
    ) -> Option<SpiderEvent> {
        self.update_with_chunks(player_pos, Some(chunks))
    }

    fn update_with_chunks(
        &mut self,
        player_pos: Vec2,
        chunks: Option<&ChunkManager>,
    ) -> Option<SpiderEvent> {
        if !self.alive {
            return None;
        }

        // Movement towards player with offset
        let mut target = player_pos - self.pos;
        let distance = target.length();

        target.x += self.target_offset.x * distance;
        target.y += self.target_offset.y * distance;
        target.normalize_mut();

        self.speed.x += target.x * ACCEL;
        self.speed.y += target.y * ACCEL;

        if self.speed.x != 0.0 || self.speed.y != 0.0 {
            self.dir = self.speed.normalize();
            if self.speed.length() > SPEED_MAX {
                self.speed = self.dir * SPEED_MAX;
            }
        }

        let new_pos = self.pos + self.speed;

        // Check obstacle collision if chunks provided
        if let Some(chunks) = chunks {
            if !chunks.collides_with_obstacle(new_pos, self.radius()) {
                self.pos = new_pos;
            } else {
                // Try sliding along obstacles
                let new_x = Vec2::new(new_pos.x, self.pos.y);
                let new_y = Vec2::new(self.pos.x, new_pos.y);

                if !chunks.collides_with_obstacle(new_x, self.radius()) {
                    self.pos.x = new_pos.x;
                } else {
                    self.speed.x *= -0.5; // Bounce back a bit
                }
                if !chunks.collides_with_obstacle(new_y, self.radius()) {
                    self.pos.y = new_pos.y;
                } else {
                    self.speed.y *= -0.5;
                }
            }
        } else {
            self.pos = new_pos;
        }

        // Check if close to player (collision will be handled externally)
        if distance < 100.0 {
            return Some(SpiderEvent::NearPlayer(distance));
        }

        None
    }

    pub fn get_closest_distance(spiders: &[Spider], player_pos: Vec2) -> Option<f32> {
        spiders
            .iter()
            .filter(|s| s.alive)
            .map(|s| s.pos.distance(player_pos))
            .min_by(|a, b| a.partial_cmp(b).unwrap())
    }

    pub fn kill(&mut self) {
        self.alive = false;
    }

    pub fn radius(&self) -> f32 {
        6.0
    }

    pub fn bump(&mut self, dir: Vec2, amount: f32) {
        self.pos += dir * amount;
        self.speed.reflect_mut(dir);
    }
}

#[derive(Debug)]
pub enum SpiderEvent {
    NearPlayer(f32),
}

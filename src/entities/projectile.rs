use crate::math::Vec2;
use crate::world::ChunkManager;

#[derive(Debug, Clone)]
pub struct Projectile {
    pub alive: bool,
    pub pos: Vec2,
    pub speed: Vec2,
    pub duration: i32,
    pub timer: i32,
    pub hostile: bool,
}

impl Projectile {
    pub fn new(pos: Vec2, speed: Vec2, duration: i32) -> Self {
        Self {
            alive: true,
            pos,
            speed,
            duration,
            timer: 0,
            hostile: true,
        }
    }

    pub fn update(&mut self) {
        if !self.alive {
            return;
        }

        self.timer += 1;
        if self.timer > self.duration {
            self.alive = false;
            return;
        }

        self.pos += self.speed;
    }

    /// Update with obstacle collision - projectile dies when hitting obstacle
    pub fn update_with_collision(&mut self, chunks: &ChunkManager) {
        if !self.alive {
            return;
        }

        self.timer += 1;
        if self.timer > self.duration {
            self.alive = false;
            return;
        }

        let new_pos = self.pos + self.speed;

        // Check if projectile hits an obstacle
        if chunks.collides_with_obstacle(new_pos, self.radius()) {
            self.alive = false;
            return;
        }

        self.pos = new_pos;
    }

    pub fn reflect(&mut self, normal: Vec2) {
        self.hostile = false;
        self.timer = 0;
        self.speed.reflect_mut(normal);
    }

    pub fn radius(&self) -> f32 {
        3.0
    }
}

pub struct ProjectilePool {
    pub projectiles: Vec<Projectile>,
    next_index: usize,
    capacity: usize,
}

impl ProjectilePool {
    pub fn new(capacity: usize) -> Self {
        Self {
            projectiles: Vec::with_capacity(capacity),
            next_index: 0,
            capacity,
        }
    }

    pub fn spawn(&mut self, pos: Vec2, speed: Vec2, duration: i32) {
        let projectile = Projectile::new(pos, speed, duration);

        if self.projectiles.len() < self.capacity {
            self.projectiles.push(projectile);
        } else {
            self.projectiles[self.next_index] = projectile;
        }
        self.next_index = (self.next_index + 1) % self.capacity.max(1);
    }

    pub fn update(&mut self) {
        for projectile in &mut self.projectiles {
            if projectile.alive {
                projectile.update();
            }
        }
    }

    /// Update all projectiles with obstacle collision
    pub fn update_with_collision(&mut self, chunks: &ChunkManager) {
        for projectile in &mut self.projectiles {
            if projectile.alive {
                projectile.update_with_collision(chunks);
            }
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &Projectile> {
        self.projectiles.iter().filter(|p| p.alive)
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Projectile> {
        self.projectiles.iter_mut().filter(|p| p.alive)
    }

    pub fn clear(&mut self) {
        self.projectiles.clear();
        self.next_index = 0;
    }
}

use crate::math::Vec2;

#[derive(Debug, Clone)]
pub struct Explosion {
    pub alive: bool,
    pub pos: Vec2,
    pub duration: i32,
    pub timer: i32,
    pub color_index: u8,
}

impl Explosion {
    pub fn new(pos: Vec2, duration: i32, timer: i32, color_index: u8) -> Self {
        Self {
            alive: true,
            pos,
            duration,
            timer,
            color_index,
        }
    }

    pub fn update(&mut self) {
        self.timer += 1;
        if self.timer > self.duration {
            self.alive = false;
        }
    }

    pub fn radius(&self) -> f32 {
        if self.timer > 0 {
            (self.timer * 2 - 1) as f32
        } else {
            0.0
        }
    }
}

pub struct ExplosionPool {
    explosions: Vec<Explosion>,
    next_index: usize,
}

impl ExplosionPool {
    pub fn new(capacity: usize) -> Self {
        Self {
            explosions: Vec::with_capacity(capacity),
            next_index: 0,
        }
    }

    pub fn spawn(&mut self, pos: Vec2, duration: i32, timer: i32, color_index: u8) {
        let explosion = Explosion::new(pos, duration, timer, color_index);

        if self.explosions.len() < self.explosions.capacity() {
            self.explosions.push(explosion);
        } else {
            self.explosions[self.next_index] = explosion;
        }
        self.next_index = (self.next_index + 1) % self.explosions.capacity().max(1);
    }

    pub fn update(&mut self) {
        for explosion in &mut self.explosions {
            if explosion.alive {
                explosion.update();
            }
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &Explosion> {
        self.explosions.iter().filter(|e| e.alive)
    }

    pub fn clear(&mut self) {
        self.explosions.clear();
        self.next_index = 0;
    }
}

use crate::math::Vec2;
use crate::world::ChunkManager;
use rand::Rng;

const WISP_SPEED: f32 = 2.6;
const WISP_WOBBLE: f32 = 0.6;
const WISP_WOBBLE_SPEED: f32 = 0.22;

#[derive(Debug, Clone)]
pub struct Wisp {
    pub id: usize,
    pub alive: bool,
    pub pos: Vec2,
    pub dir: Vec2,
    wobble_phase: f32,
}

impl Wisp {
    pub fn new_at_position<R: Rng>(id: usize, pos: Vec2, rng: &mut R) -> Self {
        let dir = Vec2::new(rng.gen::<f32>() - 0.5, rng.gen::<f32>() - 0.5).normalize();
        Self {
            id,
            alive: true,
            pos,
            dir,
            wobble_phase: rng.gen::<f32>() * std::f32::consts::TAU,
        }
    }

    pub fn update_infinite(&mut self, target: Vec2, chunks: &ChunkManager) {
        if !self.alive {
            return;
        }

        let to_target = (target - self.pos).normalize();
        let perp = Vec2::new(-to_target.y, to_target.x);
        self.wobble_phase += WISP_WOBBLE_SPEED;
        let wobble = perp * (self.wobble_phase.sin() * WISP_WOBBLE);
        let move_dir = (to_target + wobble).normalize();

        let next_pos = self.pos + move_dir * WISP_SPEED;
        if chunks.collides_with_obstacle(next_pos, self.radius()) {
            self.dir = Vec2::new(-self.dir.y, self.dir.x);
            self.pos += self.dir * (WISP_SPEED * 0.6);
            return;
        }

        self.dir = move_dir;
        self.pos = next_pos;
    }

    pub fn kill(&mut self) {
        self.alive = false;
    }

    pub fn radius(&self) -> f32 {
        6.0
    }

    pub fn bump(&mut self, dir: Vec2, amount: f32) {
        self.pos += dir * amount;
        self.dir.reflect_mut(dir);
    }
}

use crate::math::Vec2;
use crate::world::ChunkManager;

const ACCEL: f32 = 0.05;
const SPEED_MAX: f32 = 1.4;
const BODY_RADIUS: f32 = 14.0;
const STRIKE_RANGE: f32 = 130.0;
const STRIKE_OFFSET: f32 = 20.0;
const STRIKE_COOLDOWN: u32 = 120;
const STRIKE_DURATION: u32 = 18;
const LEASH_RADIUS: f32 = 150.0;
const INTERCEPT_BIAS: f32 = 0.35;
const STRIKE_BIAS: f32 = 0.55;
const MAX_HP: i32 = 7;
const MAX_TENTACLE_REACH: f32 = 200.0;
const TENTACLE_SEGMENTS: usize = 6;
const TENTACLE_COUNT: usize = 5;
const TENTACLE_SPEED_MAX: f32 = 4.8;
const TENTACLE_ACCEL: f32 = 0.35;
const TENTACLE_DAMPING: f32 = 0.82;
const TENTACLE_HIT_RADIUS: f32 = 4.5;
const TENTACLE_OVERSHOOT_FACTOR: f32 = 0.8;
const TENTACLE_WRAP_DISTANCE: f32 = 60.0;
const TENTACLE_RETRACT_FRAMES: u32 = 18;
const TENTACLE_CURL_FRAMES: u32 = 16;

#[derive(Debug, Clone)]
pub struct Tentacle {
    pub mode: u8,
    pub joints: [Vec2; TENTACLE_SEGMENTS + 1],
    pub vel: Vec2,
    pub retract_timer: u32,
    pub state: u8,
    pub state_timer: u32,
}

#[derive(Debug, Clone)]
pub struct Guardian {
    pub id: usize,
    pub alive: bool,
    pub pos: Vec2,
    pub home_pos: Vec2,
    hp: i32,
    pub speed: Vec2,
    pub dir: Vec2,
    attack_cooldown: u32,
    strike_timer: u32,
    strike_pos: Vec2,
    strike_points: [Vec2; 3],
    regen_timer: u32,
    tentacles: [Tentacle; TENTACLE_COUNT],
}

impl Guardian {
    fn safe_normalize(v: Vec2) -> Vec2 {
        if v.length() == 0.0 {
            Vec2::ZERO
        } else {
            v.normalize()
        }
    }
    pub fn new_at_position(id: usize, pos: Vec2) -> Self {
        let mut tentacles: [Tentacle; TENTACLE_COUNT] = std::array::from_fn(|i| Tentacle {
            mode: i as u8,
            joints: [pos; TENTACLE_SEGMENTS + 1],
            vel: Vec2::ZERO,
            retract_timer: 0,
            state: 0,
            state_timer: 20 + (i as u32 * 6),
        });
        for (idx, tentacle) in tentacles.iter_mut().enumerate() {
            let angle = (idx as f32) * std::f32::consts::TAU / TENTACLE_COUNT as f32;
            let offset = Vec2::new(angle.cos(), angle.sin()) * (BODY_RADIUS * 0.8);
            for joint in tentacle.joints.iter_mut() {
                *joint = pos + offset;
            }
        }
        Self {
            id,
            alive: true,
            pos,
            home_pos: pos,
            hp: MAX_HP,
            speed: Vec2::ZERO,
            dir: Vec2::new(0.0, -1.0),
            attack_cooldown: 30,
            strike_timer: 0,
            strike_pos: pos,
            strike_points: [pos; 3],
            regen_timer: 0,
            tentacles,
        }
    }

    pub fn update_infinite(&mut self, target: Vec2, chunks: &ChunkManager) {
        if !self.alive {
            return;
        }

        let dist_from_home = self.pos.distance(self.home_pos);
        let mut desired_pos = if dist_from_home > LEASH_RADIUS {
            self.home_pos
        } else {
            let bias = if self.strike_timer > 0 { STRIKE_BIAS } else { INTERCEPT_BIAS };
            self.home_pos + (target - self.home_pos) * bias
        };

        if dist_from_home > LEASH_RADIUS * 1.1 {
            desired_pos = self.home_pos;
        }

        let to_desired = desired_pos - self.pos;
        let dist = to_desired.length();
        if dist > 0.1 {
            let desired = to_desired / dist;
            self.dir = desired;
            self.speed += desired * ACCEL;
        }

        if self.speed.length() > SPEED_MAX {
            self.speed = self.speed.normalize() * SPEED_MAX;
        }

        let next = self.pos + self.speed;
        if !chunks.collides_with_obstacle(next, BODY_RADIUS) {
            self.pos = next;
        } else {
            self.speed = Vec2::ZERO;
        }
    }

    pub fn update_strike(&mut self, player_pos: Vec2, player_dir: Vec2) {
        if self.attack_cooldown > 0 {
            self.attack_cooldown = self.attack_cooldown.saturating_sub(1);
        }
        if self.strike_timer > 0 {
            self.strike_timer = self.strike_timer.saturating_sub(1);
        }

        if self.strike_timer == 0 && self.attack_cooldown == 0 {
            if self.pos.distance(player_pos) <= STRIKE_RANGE {
                let dir = if player_dir.length() > 0.1 {
                    player_dir.normalize()
                } else {
                    Vec2::new(0.0, -1.0)
                };
                let perp = Vec2::new(-dir.y, dir.x);
                let rear = player_pos - dir * STRIKE_OFFSET;
                self.strike_pos = rear;
                self.strike_points = [
                    rear + perp * 10.0,
                    rear - perp * 10.0,
                    player_pos + dir * (STRIKE_OFFSET * 0.6),
                ];
                self.strike_timer = STRIKE_DURATION;
                self.attack_cooldown = STRIKE_COOLDOWN;
            }
        }
    }

    pub fn strike_active(&self) -> bool {
        self.strike_timer > 0
    }

    pub fn strike_pos(&self) -> Vec2 {
        self.strike_pos
    }

    pub fn strike_points(&self) -> [Vec2; 3] {
        self.strike_points
    }

    pub fn radius(&self) -> f32 {
        BODY_RADIUS
    }

    pub fn strike_range(&self) -> f32 {
        STRIKE_RANGE
    }

    pub fn tentacle_reach_value() -> f32 {
        MAX_TENTACLE_REACH
    }

    pub fn tentacle_hit_radius() -> f32 {
        TENTACLE_HIT_RADIUS
    }

    pub fn tentacle_paths(&self) -> &[Tentacle; TENTACLE_COUNT] {
        &self.tentacles
    }

    pub fn tentacle_tip_positions(&self) -> [(usize, Vec2); TENTACLE_COUNT] {
        std::array::from_fn(|i| (i, self.tentacles[i].joints[TENTACLE_SEGMENTS]))
    }

    pub fn bounce_tentacle(&mut self, idx: usize, normal: Vec2) {
        if idx >= TENTACLE_COUNT {
            return;
        }
        let tentacle = &mut self.tentacles[idx];
        tentacle.vel = tentacle.vel - normal * (tentacle.vel.dot(normal));
        if tentacle.vel.length() == 0.0 {
            tentacle.vel = normal * (TENTACLE_SPEED_MAX * 0.4);
        } else {
            tentacle.vel = tentacle.vel.normalize() * (TENTACLE_SPEED_MAX * 0.6);
        }
        tentacle.retract_timer = TENTACLE_RETRACT_FRAMES;
    }

    pub fn take_damage(&mut self, amount: i32) -> bool {
        if !self.alive {
            return false;
        }
        self.hp = (self.hp - amount).max(0);
        if self.hp == 0 {
            self.alive = false;
            return true;
        }
        false
    }

    pub fn regen_tick(&mut self, should_regen: bool, regen_interval: u32) {
        if !self.alive {
            return;
        }
        if !should_regen || self.hp >= MAX_HP {
            self.regen_timer = 0;
            return;
        }
        self.regen_timer = self.regen_timer.saturating_add(1);
        if self.regen_timer >= regen_interval {
            self.hp = (self.hp + 1).min(MAX_HP);
            self.regen_timer = 0;
        }
    }

    pub fn update_tentacles(
        &mut self,
        target_positions: &[Vec2],
        chunks: &ChunkManager,
        _frame_count: u32,
    ) {
        let fallback_player = if target_positions.is_empty() {
            self.pos + Vec2::new(0.0, -1.0)
        } else {
            target_positions[0]
        };
        let seg_len = MAX_TENTACLE_REACH / TENTACLE_SEGMENTS as f32;

        for (idx, tentacle) in self.tentacles.iter_mut().enumerate() {
            let anchor_angle = (idx as f32) * std::f32::consts::TAU / TENTACLE_COUNT as f32;
            let anchor = self.pos + Vec2::new(anchor_angle.cos(), anchor_angle.sin()) * (BODY_RADIUS * 0.8);
            tentacle.joints[0] = anchor;

            let mode = tentacle.mode % 5;
            if tentacle.state_timer > 0 {
                tentacle.state_timer = tentacle.state_timer.saturating_sub(1);
            } else {
                tentacle.state = (tentacle.state + 1) % 3;
                tentacle.state_timer = match tentacle.state {
                    0 => 22,
                    1 => 14,
                    _ => TENTACLE_CURL_FRAMES,
                };
            }

            let mut closest_player = fallback_player;
            let mut closest_dist = f32::MAX;
            for pos in target_positions {
                let dist = anchor.distance(*pos);
                if dist < closest_dist {
                    closest_dist = dist;
                    closest_player = *pos;
                }
            }
            let player_dir = Self::safe_normalize(closest_player - self.pos);
            let perp = Vec2::new(-player_dir.y, player_dir.x);
            let player_dist = self.pos.distance(closest_player);
            let dir_from_guardian = Self::safe_normalize(closest_player - self.pos);

            let target = if player_dist > MAX_TENTACLE_REACH * 1.2 {
                self.pos + Vec2::new(anchor_angle.cos(), anchor_angle.sin()) * (MAX_TENTACLE_REACH * 0.6)
            } else {
                let rear = closest_player - player_dir * 24.0;
                let front = closest_player + player_dir * 16.0;
                match mode {
                    0 => front,
                    1 => rear + perp * 14.0,
                    2 => rear - perp * 14.0,
                    3 => closest_player + perp * 12.0,
                    _ => closest_player - perp * 12.0,
                }
            };
            let overshoot = closest_player + dir_from_guardian * (MAX_TENTACLE_REACH * TENTACLE_OVERSHOOT_FACTOR);
            let wrap = closest_player - player_dir * TENTACLE_WRAP_DISTANCE + perp * (if mode == 2 { -22.0 } else { 22.0 });
            let curl_center = closest_player - player_dir * (TENTACLE_WRAP_DISTANCE + 8.0);
            let curl_angle = (_frame_count as f32 * 0.6) + (idx as f32 * 1.1);
            let curl_target = curl_center + Vec2::new(curl_angle.cos(), curl_angle.sin()) * (MAX_TENTACLE_REACH * 0.35);
            let mut seek_target = if tentacle.state == 0 { overshoot } else { target };
            if tentacle.state == 0 && mode % 2 == 0 {
                seek_target = wrap;
            } else if tentacle.state == 2 {
                seek_target = curl_target;
            }

            let tip_idx = TENTACLE_SEGMENTS;
            let mut tip = tentacle.joints[tip_idx];
            if tentacle.retract_timer > 0 {
                tentacle.retract_timer = tentacle.retract_timer.saturating_sub(1);
            }

            let desired = if tentacle.retract_timer > 0 {
                self.pos + Self::safe_normalize(tip - self.pos) * (MAX_TENTACLE_REACH * 0.35)
            } else {
                seek_target
            };

            let to_desired = desired - tip;
            tentacle.vel += to_desired * TENTACLE_ACCEL;
            if tentacle.vel.length() > TENTACLE_SPEED_MAX {
                tentacle.vel = tentacle.vel.normalize() * TENTACLE_SPEED_MAX;
            }
            tentacle.vel *= TENTACLE_DAMPING;
            tip += tentacle.vel;

            if tip.distance(self.pos) > MAX_TENTACLE_REACH {
                let dir = (tip - self.pos).normalize();
                tip = self.pos + dir * MAX_TENTACLE_REACH;
                tentacle.vel = tentacle.vel - dir * (tentacle.vel.dot(dir));
            }

            if chunks.collides_with_obstacle(tip, 4.0) {
                let normal = Self::safe_normalize(tip - self.pos);
                tentacle.vel = -normal * (TENTACLE_SPEED_MAX * 0.6);
                tentacle.retract_timer = TENTACLE_RETRACT_FRAMES;
            }

            tentacle.joints[tip_idx] = tip;

            for _ in 0..2 {
                for i in 1..=TENTACLE_SEGMENTS {
                    let prev = tentacle.joints[i - 1];
                    let delta = tentacle.joints[i] - prev;
                    let dist = delta.length().max(0.001);
                    tentacle.joints[i] = prev + delta * (seg_len / dist);
                }
                tentacle.joints[tip_idx] = tip;
                for i in (0..TENTACLE_SEGMENTS).rev() {
                    let next = tentacle.joints[i + 1];
                    let delta = tentacle.joints[i] - next;
                    let dist = delta.length().max(0.001);
                    tentacle.joints[i] = next + delta * (seg_len / dist);
                }
                tentacle.joints[0] = anchor;
            }
        }
    }
}

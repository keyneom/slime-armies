use crate::input::{Input, BUTTON_ATTACK, BUTTON_PHASE};
use crate::math::Vec2;
use crate::world::ChunkManager;

// Timer constants from original game
const ATTACK_MIN: i32 = -5;
const PHASE_MIN: i32 = -10;
const SHIELD_DURATION: i32 = 120;
const SHIELD_COOLDOWN: i32 = 600;

// Movement speed - increased for larger viewport
const MOVE_SPEED: f32 = 3.5;
const PHASE_SPEED: f32 = 5.0;
const SPEED_BOOST_MULTIPLIER: f32 = 1.6;
// Creature scale for collision/interaction sizes
const CREATURE_SCALE: f32 = 2.0;

fn distance_point_to_segment(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let ap = p - a;
    let ab_len_sq = ab.length_squared();
    if ab_len_sq <= f32::EPSILON {
        return ap.length();
    }
    let mut t = ap.dot(ab) / ab_len_sq;
    if t < 0.0 {
        t = 0.0;
    } else if t > 1.0 {
        t = 1.0;
    }
    let closest = a + ab * t;
    (p - closest).length()
}

#[derive(Debug, Clone)]
pub struct Player {
    pub alive: bool,
    pub pos: Vec2,
    pub move_speed: f32,
    pub move_dir: Vec2,
    pub look_dir: Vec2,
    pub phase_dir: Vec2,
    pub phase_timer: i32,
    pub attack_timer: i32,
    pub blocking: bool,
    pub shield_timer: i32,
    pub shield_cooldown: i32,
    pub speed_boost_timer: i32,
    pub speed_boost_cooldown: i32,
}

impl Player {
    pub fn new(screen_width: u32, screen_height: u32) -> Self {
        Self {
            alive: true,
            pos: Vec2::new(screen_width as f32 / 2.0, screen_height as f32 / 2.0),
            move_speed: MOVE_SPEED,
            move_dir: Vec2::ZERO,
            look_dir: Vec2::new(0.0, -1.0),
            phase_dir: Vec2::new(0.0, -1.0),
            phase_timer: PHASE_MIN,
            attack_timer: ATTACK_MIN,
            blocking: false,
            shield_timer: 0,
            shield_cooldown: 0,
            speed_boost_timer: 0,
            speed_boost_cooldown: 0,
        }
    }

    pub fn new_at_position(pos: Vec2) -> Self {
        Self {
            alive: true,
            pos,
            move_speed: MOVE_SPEED,
            move_dir: Vec2::ZERO,
            look_dir: Vec2::new(0.0, -1.0),
            phase_dir: Vec2::new(0.0, -1.0),
            phase_timer: PHASE_MIN,
            attack_timer: ATTACK_MIN,
            blocking: false,
            shield_timer: 0,
            shield_cooldown: 0,
            speed_boost_timer: 0,
            speed_boost_cooldown: 0,
        }
    }

    pub fn update(&mut self, input: &Input) {
        if !self.alive {
            return;
        }

        self.update_timers();
        self.update_input(input);

        // Movement with no bounds (old behavior for compatibility)
        let speed = self.get_speed();
        let dir = self.get_movement_dir();
        self.pos += dir * speed;
    }

    /// Update for infinite world - checks obstacle collisions
    pub fn update_infinite(&mut self, input: &Input, chunks: &ChunkManager) {
        if !self.alive {
            return;
        }

        self.update_timers();
        self.update_input(input);

        // Movement with obstacle collision
        let speed = self.get_speed();
        let dir = self.get_movement_dir();

        let new_pos = self.pos + dir * speed;

        // When phasing, skip obstacle collision - quantum tunneling
        // Player can phase through obstacles and enemies, may end up "stuck"
        // but can always phase out again
        if self.is_phasing() || !chunks.collides_with_obstacle(new_pos, 4.5 * CREATURE_SCALE) {
            self.pos = new_pos;
        } else {
            // Try sliding along obstacles (normal movement only)
            let new_x = Vec2::new(new_pos.x, self.pos.y);
            let new_y = Vec2::new(self.pos.x, new_pos.y);

            if !chunks.collides_with_obstacle(new_x, 4.5 * CREATURE_SCALE) {
                self.pos.x = new_pos.x;
            }
            if !chunks.collides_with_obstacle(new_y, 4.5 * CREATURE_SCALE) {
                self.pos.y = new_pos.y;
            }
        }
    }

    fn update_timers(&mut self) {
        if self.attack_timer > ATTACK_MIN {
            self.attack_timer -= 1;
        }
        if self.phase_timer > PHASE_MIN {
            self.phase_timer -= 1;
        }
        if self.shield_timer > 0 {
            self.shield_timer -= 1;
        }
        if self.shield_cooldown > 0 {
            self.shield_cooldown -= 1;
        }
        if self.speed_boost_timer > 0 {
            self.speed_boost_timer -= 1;
        }
        if self.speed_boost_cooldown > 0 {
            self.speed_boost_cooldown -= 1;
        }
    }

    fn update_input(&mut self, input: &Input) {
        // Attack - triggered on button release
        if self.attack_timer <= ATTACK_MIN && input.is_released(BUTTON_ATTACK) {
            self.attack_timer = 5;
        }

        // Direction
        self.move_dir = input.axis;
        if self.move_dir.x != 0.0 || self.move_dir.y != 0.0 {
            if self.phase_timer <= 0 {
                self.phase_dir = self.move_dir;
            }
            if self.attack_timer <= 0 && !input.is_down(BUTTON_ATTACK) {
                self.look_dir = self.move_dir;
            }
        }

        // Phase (quantum tunneling)
        if self.phase_timer <= PHASE_MIN && input.is_pressed(BUTTON_PHASE) {
            self.phase_timer = 15;
        }

        // Block
        self.blocking =
            self.attack_timer <= 0 && self.phase_timer <= 0 && input.is_down(BUTTON_ATTACK);
    }

    fn get_speed(&self) -> f32 {
        let base = if self.phase_timer > 0 {
            PHASE_SPEED
        } else {
            self.move_speed
        };
        if self.speed_boost_timer > 0 {
            base * SPEED_BOOST_MULTIPLIER
        } else {
            base
        }
    }

    fn get_movement_dir(&self) -> Vec2 {
        if self.phase_timer > 0 {
            self.phase_dir
        } else {
            self.move_dir
        }
    }

    pub fn is_attacking(&self) -> bool {
        self.attack_timer > 0
    }

    pub fn is_phasing(&self) -> bool {
        self.phase_timer > 0
    }

    pub fn is_shielded(&self) -> bool {
        self.shield_timer > 0
    }

    pub fn try_activate_shield(&mut self) -> bool {
        if self.shield_cooldown > 0 || self.shield_timer > 0 {
            return false;
        }
        self.shield_timer = SHIELD_DURATION;
        self.shield_cooldown = SHIELD_COOLDOWN;
        true
    }

    pub fn try_activate_speed_boost(&mut self, duration: i32, cooldown: i32) -> bool {
        if self.speed_boost_cooldown > 0 || self.speed_boost_timer > 0 {
            return false;
        }
        self.speed_boost_timer = duration;
        self.speed_boost_cooldown = cooldown;
        true
    }

    /// Body collision - original uses 4.5 + radius
    pub fn collide_body(&self, target: Vec2, radius: f32) -> bool {
        if !self.alive || self.is_phasing() || self.is_shielded() {
            return false;
        }
        self.pos.distance(target) < 4.5 * CREATURE_SCALE + radius
    }

    /// Block collision - block_pos at look_dir * 2, check 7.5 + radius
    pub fn collide_block(&self, target: Vec2, radius: f32) -> bool {
        if !self.alive {
            return false;
        }
        // Bubble shield blocks from all sides while active.
        if self.is_shielded() {
            return self.pos.distance(target) < 5.5 * CREATURE_SCALE + radius;
        }
        if !self.blocking {
            return false;
        }
        let look = self.look_dir.normalize();
        if look.length_squared() == 0.0 {
            return false;
        }

        let perp = Vec2::new(look.y, -look.x);
        let scale = CREATURE_SCALE;

        let s1 = self.pos + (look * (5.0 * scale) - perp * (7.0 * scale));
        let e1 = self.pos + (look * (5.0 * scale) + perp * (7.0 * scale));
        let s2 = self.pos + (look * (6.0 * scale) - perp * (4.0 * scale));
        let e2 = self.pos + (look * (6.0 * scale) + perp * (4.0 * scale));

        let thickness = 1.2 * scale + radius;
        let d1 = distance_point_to_segment(target, s1, e1);
        let d2 = distance_point_to_segment(target, s2, e2);

        d1 <= thickness || d2 <= thickness
    }

    /// Attack collision - attack_pos at look_dir * 5, check 8.5 + radius
    pub fn collide_attack(&self, target: Vec2, radius: f32) -> bool {
        if !self.alive || !self.is_attacking() {
            return false;
        }
        let attack_pos = self.pos + self.look_dir * (5.0 * CREATURE_SCALE);
        attack_pos.distance(target) < 8.5 * CREATURE_SCALE + radius
    }

    pub fn kill(&mut self) {
        self.alive = false;
    }

    pub fn apply_net_state(&mut self, state: &crate::net::PlayerState) {
        self.pos = state.pos();
        self.look_dir = state.look_dir();
        self.move_dir = state.move_dir();
        self.alive = state.is_alive();
        self.blocking = state.is_blocking();
        self.attack_timer = if state.is_attacking() { 1 } else { ATTACK_MIN };
        self.phase_timer = if state.is_phasing() { 1 } else { PHASE_MIN };
        self.shield_timer = if state.is_shielded() { 1 } else { 0 };
        if self.move_dir.x != 0.0 || self.move_dir.y != 0.0 {
            self.phase_dir = self.move_dir;
        }
    }
}

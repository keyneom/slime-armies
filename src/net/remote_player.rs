use crate::math::Vec2;
use crate::net::PlayerState;

/// Represents a remote player with interpolation
#[derive(Debug, Clone)]
pub struct RemotePlayer {
    pub name: String,
    pub pos: Vec2,
    pub look_dir: Vec2,
    pub move_dir: Vec2,
    pub alive: bool,
    pub attacking: bool,
    pub blocking: bool,
    pub phasing: bool,
    pub shielded: bool,

    // For interpolation
    target_pos: Vec2,
    prev_pos: Vec2,
    interpolation_t: f32,
    last_update_frame: u32,
}

impl RemotePlayer {
    pub fn new(name: String, state: &PlayerState, frame: u32) -> Self {
        let pos = state.pos();
        Self {
            name,
            pos,
            look_dir: state.look_dir(),
            move_dir: state.move_dir(),
            alive: state.is_alive(),
            attacking: state.is_attacking(),
            blocking: state.is_blocking(),
            phasing: state.is_phasing(),
            shielded: state.is_shielded(),
            target_pos: pos,
            prev_pos: pos,
            interpolation_t: 1.0,
            last_update_frame: frame,
        }
    }

    /// Update with new state from network
    pub fn update_state(&mut self, state: &PlayerState, frame: u32) {
        self.prev_pos = self.pos;
        self.target_pos = state.pos();
        self.look_dir = state.look_dir();
        self.move_dir = state.move_dir();
        self.alive = state.is_alive();
        self.attacking = state.is_attacking();
        self.blocking = state.is_blocking();
        self.phasing = state.is_phasing();
        self.shielded = state.is_shielded();
        self.interpolation_t = 0.0;
        self.last_update_frame = frame;
    }

    pub fn apply_predicted_state(&mut self, state: &PlayerState) {
        let pos = state.pos();
        self.prev_pos = pos;
        self.target_pos = pos;
        self.pos = pos;
        self.look_dir = state.look_dir();
        self.move_dir = state.move_dir();
        self.alive = state.is_alive();
        self.attacking = state.is_attacking();
        self.blocking = state.is_blocking();
        self.phasing = state.is_phasing();
        self.shielded = state.is_shielded();
        self.interpolation_t = 1.0;
    }

    /// Interpolate position each frame
    pub fn update(&mut self) {
        // Smooth interpolation over ~6 frames
        self.interpolation_t = (self.interpolation_t + 0.17).min(1.0);

        // Linear interpolation
        self.pos = Vec2::new(
            self.prev_pos.x + (self.target_pos.x - self.prev_pos.x) * self.interpolation_t,
            self.prev_pos.y + (self.target_pos.y - self.prev_pos.y) * self.interpolation_t,
        );
    }

    /// Check if this player hasn't been updated for too long (disconnected)
    pub fn is_stale(&self, current_frame: u32) -> bool {
        current_frame.saturating_sub(self.last_update_frame) > 180 // ~3 seconds at 60fps
    }

    pub fn last_update_frame(&self) -> u32 {
        self.last_update_frame
    }
}

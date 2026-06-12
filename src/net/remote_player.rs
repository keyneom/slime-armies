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
    target_velocity: Vec2,
    avg_update_delta_frames: f32,
    last_state_frame: u32,
    last_update_frame: u32,
    /// Last authoritative state exactly as received (sender frame domain).
    /// Predictions anchor on this — never on the smoothed display fields.
    last_state: PlayerState,
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
            target_velocity: Vec2::ZERO,
            avg_update_delta_frames: 6.0,
            last_state_frame: state.sim_frame(),
            last_update_frame: frame,
            last_state: *state,
        }
    }

    /// Update with new state from network
    pub fn update_state(&mut self, state: &PlayerState, frame: u32) {
        // Drop stale/duplicate state to avoid out-of-order jitter and
        // interpolation restarts on identical relay copies.
        if state.sim_frame() <= self.last_state_frame {
            self.last_update_frame = frame;
            return;
        }
        let next_target = state.pos();
        let sim_delta = state
            .sim_frame()
            .saturating_sub(self.last_state_frame)
            .max(1) as f32;
        let observed_velocity = (next_target - self.target_pos) / sim_delta;
        self.target_velocity = self.target_velocity * 0.55 + observed_velocity * 0.45;
        let arrival_delta = frame.saturating_sub(self.last_update_frame).max(1) as f32;
        self.avg_update_delta_frames =
            (self.avg_update_delta_frames * 0.75 + arrival_delta * 0.25).clamp(2.0, 14.0);
        self.target_pos = next_target;
        self.look_dir = state.look_dir();
        self.move_dir = state.move_dir();
        self.alive = state.is_alive();
        self.attacking = state.is_attacking();
        self.blocking = state.is_blocking();
        self.phasing = state.is_phasing();
        self.shielded = state.is_shielded();
        self.last_state_frame = state.sim_frame();
        self.last_update_frame = frame;
        self.last_state = *state;
    }

    /// The raw last received state, for anchoring input-replay predictions.
    pub fn last_authoritative_state(&self) -> PlayerState {
        self.last_state
    }

    /// Adopt an input-replay prediction as the smoothing target. The display
    /// position is NOT set here: update() glides toward the target, so a
    /// prediction correction never teleports the player on screen (the old
    /// hard `pos = predicted` reset wiped the interpolator every tick).
    pub fn apply_predicted_state(&mut self, state: &PlayerState) {
        self.target_pos = state.pos();
        self.target_velocity = Vec2::ZERO;
        self.look_dir = state.look_dir();
        self.move_dir = state.move_dir();
        self.alive = state.is_alive();
        self.attacking = state.is_attacking();
        self.blocking = state.is_blocking();
        self.phasing = state.is_phasing();
        self.shielded = state.is_shielded();
    }

    /// Interpolate position each frame
    pub fn update(&mut self) {
        const SNAP_DIST: f32 = 300.0;
        let predict_frames = (self.avg_update_delta_frames * 0.5).clamp(0.0, 4.0);
        let predicted_target = self.target_pos + self.target_velocity * predict_frames;
        // Genuine relocations (respawns, phases) snap; everything else glides.
        if (predicted_target - self.pos).length() > SNAP_DIST {
            self.pos = predicted_target;
        } else {
            let blend = (2.4 / self.avg_update_delta_frames.max(2.0)).clamp(0.18, 0.60);
            self.pos = self.pos.lerp(predicted_target, blend);
        }

        if self.move_dir.length_squared() == 0.0 && self.target_velocity.length_squared() > 0.0001 {
            self.move_dir = self.target_velocity.normalize();
        }
    }

    /// Check if this player hasn't been updated for too long (disconnected)
    pub fn is_stale(&self, current_frame: u32) -> bool {
        current_frame.saturating_sub(self.last_update_frame) > 180 // ~3 seconds at 60fps
    }

    pub fn last_update_frame(&self) -> u32 {
        self.last_update_frame
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_stale_and_duplicate_frames() {
        let initial = PlayerState::new(
            10,
            Vec2::new(0.0, 0.0),
            Vec2::RIGHT,
            Vec2::RIGHT,
            true,
            false,
            false,
            false,
            false,
        );
        let mut remote = RemotePlayer::new("test".to_string(), &initial, 10);

        let newer = PlayerState::new(
            12,
            Vec2::new(100.0, 0.0),
            Vec2::RIGHT,
            Vec2::RIGHT,
            true,
            false,
            false,
            false,
            false,
        );
        remote.update_state(&newer, 20);
        remote.update();
        let pos_after_new = remote.pos;

        // Duplicate frame should be ignored.
        remote.update_state(&newer, 21);
        remote.update();
        let pos_after_duplicate = remote.pos;

        // Older frame should be ignored.
        let stale = PlayerState::new(
            11,
            Vec2::new(-100.0, 0.0),
            Vec2::LEFT,
            Vec2::LEFT,
            true,
            false,
            false,
            false,
            false,
        );
        remote.update_state(&stale, 22);
        remote.update();
        let pos_after_stale = remote.pos;

        assert!(pos_after_new.x >= 0.0);
        assert!(pos_after_duplicate.x >= pos_after_new.x);
        assert!(pos_after_stale.x >= pos_after_duplicate.x);
    }
}

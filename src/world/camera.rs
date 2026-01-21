use crate::math::Vec2;

/// Camera that follows the player and converts between world and screen coordinates
pub struct Camera {
    /// Position in world coordinates (center of camera)
    pub pos: Vec2,
    /// Screen dimensions
    pub screen_width: f32,
    pub screen_height: f32,
    /// Zoom factor (>1.0 zooms in, <1.0 zooms out)
    pub zoom: f32,
}

impl Camera {
    pub fn new(screen_width: u32, screen_height: u32, zoom: f32) -> Self {
        Self {
            pos: Vec2::ZERO,
            screen_width: screen_width as f32,
            screen_height: screen_height as f32,
            zoom,
        }
    }

    /// Update camera to follow a target (usually player position)
    pub fn follow(&mut self, target: Vec2) {
        // Instant follow for now - could add smoothing later
        self.pos = target;
    }

    /// Convert world coordinates to screen coordinates
    pub fn world_to_screen(&self, world_pos: Vec2) -> Vec2 {
        let offset = (world_pos - self.pos) * self.zoom;
        Vec2::new(
            offset.x + self.screen_width / 2.0,
            offset.y + self.screen_height / 2.0,
        )
    }

    /// Convert screen coordinates to world coordinates
    pub fn screen_to_world(&self, screen_pos: Vec2) -> Vec2 {
        let offset = Vec2::new(
            screen_pos.x - self.screen_width / 2.0,
            screen_pos.y - self.screen_height / 2.0,
        );
        self.pos + offset / self.zoom
    }

    /// Check if a world position is visible on screen (with padding)
    pub fn is_visible(&self, world_pos: Vec2, radius: f32) -> bool {
        let screen_pos = self.world_to_screen(world_pos);
        screen_pos.x + radius >= 0.0
            && screen_pos.x - radius <= self.screen_width
            && screen_pos.y + radius >= 0.0
            && screen_pos.y - radius <= self.screen_height
    }

    /// Get the visible world bounds (min_x, min_y, max_x, max_y)
    pub fn visible_bounds(&self) -> (f32, f32, f32, f32) {
        let half_w = self.screen_width / (2.0 * self.zoom);
        let half_h = self.screen_height / (2.0 * self.zoom);
        (
            self.pos.x - half_w,
            self.pos.y - half_h,
            self.pos.x + half_w,
            self.pos.y + half_h,
        )
    }
}

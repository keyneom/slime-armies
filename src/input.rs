use crate::math::Vec2;

pub const BUTTON_UP: u8 = 0b0000_0001;
pub const BUTTON_DOWN: u8 = 0b0000_0010;
pub const BUTTON_LEFT: u8 = 0b0000_0100;
pub const BUTTON_RIGHT: u8 = 0b0000_1000;
pub const BUTTON_ATTACK: u8 = 0b0001_0000;  // Z key / Space
pub const BUTTON_PHASE: u8 = 0b0010_0000;   // X key / Shift (quantum phase)
pub const BUTTON_MAP: u8 = 0b0100_0000;     // M key / Map toggle

#[derive(Debug, Clone, Default)]
pub struct Input {
    pub down: u8,
    prev: u8,
    pub pressed: u8,
    pub released: u8,
    pub axis: Vec2,
}

impl Input {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_raw(raw: u8, prev_raw: u8) -> Self {
        let mut input = Self {
            down: raw,
            prev: prev_raw,
            pressed: 0,
            released: 0,
            axis: Vec2::ZERO,
        };
        input.end_frame();
        input
    }

    pub fn key_down(&mut self, code: &str) {
        let button = key_to_button(code);
        self.down |= button;
    }

    pub fn key_up(&mut self, code: &str) {
        let button = key_to_button(code);
        self.down &= !button;
    }

    pub fn end_frame(&mut self) {
        self.pressed = self.down & (self.down ^ self.prev);
        self.released = self.prev & (self.down ^ self.prev);
        self.prev = self.down;

        // Update axis from directional buttons
        self.axis = Vec2::ZERO;
        if self.is_down(BUTTON_LEFT) {
            self.axis.x = -1.0;
        } else if self.is_down(BUTTON_RIGHT) {
            self.axis.x = 1.0;
        }
        if self.is_down(BUTTON_UP) {
            self.axis.y = -1.0;
        } else if self.is_down(BUTTON_DOWN) {
            self.axis.y = 1.0;
        }

        // Normalize diagonal movement
        if self.axis.x != 0.0 && self.axis.y != 0.0 {
            self.axis.normalize_mut();
        }
    }

    pub fn is_down(&self, button: u8) -> bool {
        self.down & button != 0
    }

    pub fn is_pressed(&self, button: u8) -> bool {
        self.pressed & button != 0
    }

    pub fn is_released(&self, button: u8) -> bool {
        self.released & button != 0
    }

    pub fn get_raw(&self) -> u8 {
        self.down
    }
}

fn key_to_button(code: &str) -> u8 {
    match code {
        "ArrowUp" | "KeyW" => BUTTON_UP,
        "ArrowDown" | "KeyS" => BUTTON_DOWN,
        "ArrowLeft" | "KeyA" => BUTTON_LEFT,
        "ArrowRight" | "KeyD" => BUTTON_RIGHT,
        "KeyZ" | "Space" => BUTTON_ATTACK,
        "KeyX" | "ShiftLeft" | "ShiftRight" => BUTTON_PHASE,
        "KeyM" => BUTTON_MAP,
        _ => 0,
    }
}

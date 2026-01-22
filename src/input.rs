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
    touch_down: u8,
    touch_axis: Option<Vec2>,
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
            touch_down: 0,
            touch_axis: None,
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
        let effective_down = self.down | self.touch_down;
        self.pressed = effective_down & (effective_down ^ self.prev);
        self.released = self.prev & (effective_down ^ self.prev);
        self.prev = effective_down;

        // Update axis from directional buttons
        self.axis = Vec2::ZERO;
        if let Some(axis) = self.touch_axis {
            self.axis = axis;
        } else {
            if effective_down & BUTTON_LEFT != 0 {
                self.axis.x = -1.0;
            } else if effective_down & BUTTON_RIGHT != 0 {
                self.axis.x = 1.0;
            }
            if effective_down & BUTTON_UP != 0 {
                self.axis.y = -1.0;
            } else if effective_down & BUTTON_DOWN != 0 {
                self.axis.y = 1.0;
            }
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
        self.down | self.touch_down
    }

    pub fn set_touch_state(&mut self, axis: Option<Vec2>, down_mask: u8) {
        self.touch_axis = axis;
        self.touch_down = down_mask;
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

// Allow dead code for items that will be used in future phases (GGRS, multiplayer, etc.)
#![allow(dead_code)]

use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use std::cell::RefCell;
use std::rc::Rc;

mod math;
mod input;
mod render;
mod game;
mod entities;
mod world;
mod net;
mod audio;

use game::{Game, Scene};
use input::Input;
use render::Renderer;
use net::{NetworkSession, NetworkState, PlayerState};
use audio::Audio;

// Default signaling server - using Johan Helsing's public matchbox server
// This is the same server used by Extreme Bevy and other matchbox games
const DEFAULT_SIGNALING_SERVER: &str = "wss://match-0-13.helsing.studio";

// Configurable server URL (can be set via JS)
thread_local! {
    static SIGNALING_SERVER: RefCell<String> = RefCell::new(DEFAULT_SIGNALING_SERVER.to_string());
}

// Thread-local storage for game state accessible from JS
thread_local! {
    static GAME_STATE: RefCell<Option<Rc<RefCell<GameState>>>> = RefCell::new(None);
    // Separate input buffer to avoid borrow conflicts - event handlers write here,
    // game loop reads and clears each frame
    static INPUT_BUFFER: RefCell<InputBuffer> = RefCell::new(InputBuffer::new());
    // Debug command queue (fed via JS console or tooling)
    static DEBUG_COMMANDS: RefCell<Vec<String>> = RefCell::new(Vec::new());
    static TOUCH_STATE: RefCell<TouchState> = RefCell::new(TouchState::new());
    static MOBILE_INPUT: RefCell<Option<web_sys::HtmlInputElement>> = RefCell::new(None);
}

/// Buffer for input events that event handlers can write to without conflicting
/// with the main game state borrow
struct InputBuffer {
    keys_down: Vec<String>,
    keys_up: Vec<String>,
    chars: Vec<char>,
    backspace: bool,
    escape: bool,
    enter: bool,
    tab: bool,
    mute_toggle: bool,  // M key to toggle audio mute
    click: Option<(f64, f64)>,
}

struct TouchLayout {
    stick_center: crate::math::Vec2,
    stick_radius: f64,
    attack_center: crate::math::Vec2,
    phase_center: crate::math::Vec2,
    button_radius: f64,
    map_center: crate::math::Vec2,
    list_center: crate::math::Vec2,
    chat_center: crate::math::Vec2,
    zoom_in_center: crate::math::Vec2,
    zoom_out_center: crate::math::Vec2,
    top_button_radius: f64,
}

fn touch_layout(width: f64, height: f64) -> TouchLayout {
    let stick_center = crate::math::Vec2::new(90.0, (height - 90.0) as f32);
    let stick_radius = 60.0;
    let button_radius = 32.0;
    let attack_center = crate::math::Vec2::new((width - 90.0) as f32, (height - 120.0) as f32);
    let phase_center = crate::math::Vec2::new((width - 160.0) as f32, (height - 60.0) as f32);
    let top_button_radius = 26.0;
    let map_center = crate::math::Vec2::new((width - 50.0) as f32, 40.0);
    let list_center = crate::math::Vec2::new((width - 100.0) as f32, 40.0);
    let chat_center = crate::math::Vec2::new((width * 0.5) as f32, 32.0);
    let zoom_in_center = crate::math::Vec2::new((width - 50.0) as f32, (height - 220.0) as f32);
    let zoom_out_center = crate::math::Vec2::new((width - 110.0) as f32, (height - 220.0) as f32);

    TouchLayout {
        stick_center,
        stick_radius,
        attack_center,
        phase_center,
        button_radius,
        map_center,
        list_center,
        chat_center,
        zoom_in_center,
        zoom_out_center,
        top_button_radius,
    }
}

struct TouchState {
    joystick_id: Option<i32>,
    joystick_center: crate::math::Vec2,
    joystick_axis: crate::math::Vec2,
    attack_id: Option<i32>,
    phase_id: Option<i32>,
    map_tap_frames: u8,
    list_tap_frames: u8,
    chat_tap_frames: u8,
    map_drag_id: Option<i32>,
    map_drag_last: crate::math::Vec2,
    map_drag_delta: crate::math::Vec2,
    map_drag_distance: f32,
    map_tap_candidate: bool,
    map_tap_pos: crate::math::Vec2,
    pinch_ids: Option<(i32, i32)>,
    pinch_start_distance: f32,
    list_drag_id: Option<i32>,
    list_drag_last: crate::math::Vec2,
    list_scroll_delta: f32,
    zoom_in_tap_frames: u8,
    zoom_out_tap_frames: u8,
}

impl TouchState {
    fn new() -> Self {
        Self {
            joystick_id: None,
            joystick_center: crate::math::Vec2::ZERO,
            joystick_axis: crate::math::Vec2::ZERO,
            attack_id: None,
            phase_id: None,
            map_tap_frames: 0,
            list_tap_frames: 0,
            chat_tap_frames: 0,
            map_drag_id: None,
            map_drag_last: crate::math::Vec2::ZERO,
            map_drag_delta: crate::math::Vec2::ZERO,
            map_drag_distance: 0.0,
            map_tap_candidate: false,
            map_tap_pos: crate::math::Vec2::ZERO,
            pinch_ids: None,
            pinch_start_distance: 0.0,
            list_drag_id: None,
            list_drag_last: crate::math::Vec2::ZERO,
            list_scroll_delta: 0.0,
            zoom_in_tap_frames: 0,
            zoom_out_tap_frames: 0,
        }
    }
}

impl InputBuffer {
    fn new() -> Self {
        Self {
            keys_down: Vec::new(),
            keys_up: Vec::new(),
            chars: Vec::new(),
            backspace: false,
            escape: false,
            enter: false,
            tab: false,
            mute_toggle: false,
            click: None,
        }
    }

    fn clear(&mut self) {
        self.keys_down.clear();
        self.keys_up.clear();
        self.chars.clear();
        self.backspace = false;
        self.escape = false;
        self.enter = false;
        self.tab = false;
        self.mute_toggle = false;
        self.click = None;
    }
}

fn load_saved_player_name() -> Option<String> {
    let window = web_sys::window()?;
    let storage = window.local_storage().ok()??;
    storage.get_item("slime_player_name").ok().flatten()
}

fn load_saved_room_code() -> Option<String> {
    let window = web_sys::window()?;
    let storage = window.local_storage().ok()??;
    storage.get_item("slime_room_code").ok().flatten()
}

fn save_player_name(name: &str) {
    if let Some(window) = web_sys::window() {
        if let Ok(Some(storage)) = window.local_storage() {
            let _ = storage.set_item("slime_player_name", name);
        }
    }
}

fn save_room_code(code: &str) {
    if let Some(window) = web_sys::window() {
        if let Ok(Some(storage)) = window.local_storage() {
            let _ = storage.set_item("slime_room_code", code);
        }
    }
}

#[wasm_bindgen(start)]
pub fn main() -> Result<(), JsValue> {
    console_error_panic_hook::set_once();

    let window = web_sys::window().expect("no global window");
    let document = window.document().expect("no document");

    let canvas = document
        .get_element_by_id("canvas")
        .expect("no canvas element")
        .dyn_into::<web_sys::HtmlCanvasElement>()?;

    // Set canvas size (matching original WASM-4 style but larger)
    let width = 800;
    let height = 600;
    canvas.set_width(width);
    canvas.set_height(height);

    let renderer = Renderer::new(&canvas)?;
    let input = Input::new();
    let mut game = Game::new(width, height);
    let mut network = NetworkSession::new();
    if let Some(saved_name) = load_saved_player_name() {
        game.player_name = saved_name.clone();
        network.set_player_name(&saved_name);
    }
    if let Some(saved_room) = load_saved_room_code() {
        game.room_code_input = saved_room;
    }
    let audio = Audio::new();

    // Viewport width <= 768: show mobile controls (touch joystick, etc.). Unknown width → desktop.
    let is_mobile = window
        .inner_width()
        .ok()
        .and_then(|w| w.as_f64())
        .map(|w| w <= 768.0)
        .unwrap_or(false);
    game.set_mobile_mode(is_mobile);

    if let Some(input_el) = document.get_element_by_id("mobile-input") {
        if let Ok(input_el) = input_el.dyn_into::<web_sys::HtmlInputElement>() {
            MOBILE_INPUT.with(|cell| {
                *cell.borrow_mut() = Some(input_el);
            });
        }
    }

    let state = Rc::new(RefCell::new(GameState {
        game,
        input,
        renderer,
        network,
        audio,
        send_counter: 0,
        input_send_counter: 0,
        last_supernode_id: None,
    }));

    // Store state in thread-local for JS access
    GAME_STATE.with(|gs| {
        *gs.borrow_mut() = Some(Rc::clone(&state));
    });

    // Set up keyboard and mouse event listeners
    setup_input(&window, Rc::clone(&state), &canvas)?;

    // Start game loop
    start_game_loop(window, state)?;

    Ok(())
}

/// Create a new room and return the room code
#[wasm_bindgen]
pub fn create_room() -> String {
    GAME_STATE.with(|gs| {
        if let Some(state) = gs.borrow().as_ref() {
            let mut state_ref = state.borrow_mut();
            let server = signaling_server_url();
            state_ref.network.create_room(&server)
        } else {
            String::new()
        }
    })
}

/// Join an existing room by code
#[wasm_bindgen]
pub fn join_room(room_code: &str) {
    GAME_STATE.with(|gs| {
        if let Some(state) = gs.borrow().as_ref() {
            let mut state_ref = state.borrow_mut();
            let server = signaling_server_url();
            state_ref.network.join_room(&server, room_code);
        }
    });
}

/// Disconnect from current room
#[wasm_bindgen]
pub fn disconnect() {
    GAME_STATE.with(|gs| {
        if let Some(state) = gs.borrow().as_ref() {
            let mut state_ref = state.borrow_mut();
            state_ref.network.disconnect();
        }
    });
}

/// Get current network state as string
#[wasm_bindgen]
pub fn get_network_state() -> String {
    GAME_STATE.with(|gs| {
        if let Some(state) = gs.borrow().as_ref() {
            let state_ref = state.borrow();
            match &state_ref.network.state {
                NetworkState::Disconnected => "disconnected".to_string(),
                NetworkState::Connecting => "connecting".to_string(),
                NetworkState::WaitingForPeers => "waiting".to_string(),
                NetworkState::Connected => "connected".to_string(),
                NetworkState::Error(e) => format!("error:{}", e),
            }
        } else {
            "uninitialized".to_string()
        }
    })
}

/// Get current room code
#[wasm_bindgen]
pub fn get_room_code() -> String {
    GAME_STATE.with(|gs| {
        if let Some(state) = gs.borrow().as_ref() {
            let state_ref = state.borrow();
            state_ref.network.room_code.clone()
        } else {
            String::new()
        }
    })
}

/// Get number of connected players (including self)
#[wasm_bindgen]
pub fn get_player_count() -> usize {
    GAME_STATE.with(|gs| {
        if let Some(state) = gs.borrow().as_ref() {
            let state_ref = state.borrow();
            state_ref.network.peer_count() + 1
        } else {
            0
        }
    })
}

/// Set the local player's name
#[wasm_bindgen]
pub fn set_player_name(name: &str) {
    GAME_STATE.with(|gs| {
        if let Some(state) = gs.borrow().as_ref() {
            let mut state_ref = state.borrow_mut();
            state_ref.network.set_player_name(name);
        }
    });
}

/// Get the local player's name
#[wasm_bindgen]
pub fn get_player_name() -> String {
    GAME_STATE.with(|gs| {
        if let Some(state) = gs.borrow().as_ref() {
            let state_ref = state.borrow();
            state_ref.network.local_player_name.clone()
        } else {
            String::new()
        }
    })
}

/// Set the signaling server URL (for multiplayer)
/// Examples: "ws://localhost:3536" for local, "wss://your-server.com" for production
#[wasm_bindgen]
pub fn set_signaling_server(url: &str) {
    SIGNALING_SERVER.with(|s| {
        *s.borrow_mut() = url.to_string();
    });
    web_sys::console::log_1(&format!("Signaling server set to: {}", url).into());
}

/// Get the current signaling server URL
#[wasm_bindgen]
pub fn get_signaling_server() -> String {
    SIGNALING_SERVER.with(|s| s.borrow().clone())
}

/// Toggle audio mute on/off
#[wasm_bindgen]
pub fn toggle_mute() {
    GAME_STATE.with(|gs| {
        if let Some(state) = gs.borrow().as_ref() {
            let mut state_ref = state.borrow_mut();
            state_ref.audio.toggle_mute();
        }
    });
}

/// Set audio volume (0.0 to 1.0)
#[wasm_bindgen]
pub fn set_audio_volume(volume: f32) {
    GAME_STATE.with(|gs| {
        if let Some(state) = gs.borrow().as_ref() {
            let mut state_ref = state.borrow_mut();
            state_ref.audio.set_volume(volume);
        }
    });
}

/// Get current audio mute status
#[wasm_bindgen]
pub fn is_muted() -> bool {
    GAME_STATE.with(|gs| {
        if let Some(state) = gs.borrow().as_ref() {
            let state_ref = state.borrow();
            state_ref.audio.muted
        } else {
            false
        }
    })
}

/// Get current audio volume
#[wasm_bindgen]
pub fn get_audio_volume() -> f32 {
    GAME_STATE.with(|gs| {
        if let Some(state) = gs.borrow().as_ref() {
            let state_ref = state.borrow();
            state_ref.audio.volume
        } else {
            0.8
        }
    })
}

/// Enqueue a debug command to be processed on the next frame.
/// Example (in console): `slime.send_debug_command("teleport 100 100")`
#[wasm_bindgen]
pub fn send_debug_command(command: String) {
    DEBUG_COMMANDS.with(|queue| {
        queue.borrow_mut().push(command);
    });
}

/// Helper function to get the signaling server URL
fn signaling_server_url() -> String {
    SIGNALING_SERVER.with(|s| s.borrow().clone())
}

struct GameState {
    game: Game,
    input: Input,
    renderer: Renderer,
    network: NetworkSession,
    audio: Audio,
    send_counter: u32, // Send network updates every N frames
    input_send_counter: u32, // Send input frames every N frames
    last_supernode_id: Option<matchbox_socket::PeerId>,
}

fn setup_input(window: &web_sys::Window, state: Rc<RefCell<GameState>>, canvas: &web_sys::HtmlCanvasElement) -> Result<(), JsValue> {
    // Keydown handler - writes to INPUT_BUFFER instead of directly to state
    let keydown = Closure::<dyn FnMut(_)>::new(move |event: web_sys::KeyboardEvent| {
        let code = event.code();
        let key = event.key();

        event.prevent_default();

        INPUT_BUFFER.with(|buf| {
            let mut buf = buf.borrow_mut();

            // Special keys by code
            match code.as_str() {
                "Backspace" => buf.backspace = true,
                "Escape" => buf.escape = true,
                "Enter" => buf.enter = true,
                "Tab" => buf.tab = true,
                // Arrow keys and modifiers - only add to keys_down, no character
                "ArrowUp" | "ArrowDown" | "ArrowLeft" | "ArrowRight" |
                "ShiftLeft" | "ShiftRight" | "ControlLeft" | "ControlRight" |
                "AltLeft" | "AltRight" | "MetaLeft" | "MetaRight" => {
                    buf.keys_down.push(code);
                }
                _ => {
                    // Regular keys go to keys_down
                    buf.keys_down.push(code);

                    // Only capture single characters for text input
                    // key.len() == 1 filters out special keys like "Shift", "ArrowUp", etc.
                    if key.len() == 1 {
                        if let Some(c) = key.chars().next() {
                            if c.is_ascii() && !c.is_ascii_control() {
                                buf.chars.push(c);
                            }
                        }
                    }
                }
            }
        });
    });
    window.add_event_listener_with_callback("keydown", keydown.as_ref().unchecked_ref())?;
    keydown.forget();

    // Keyup handler - writes to INPUT_BUFFER
    let keyup = Closure::<dyn FnMut(_)>::new(move |event: web_sys::KeyboardEvent| {
        event.prevent_default();
        INPUT_BUFFER.with(|buf| {
            buf.borrow_mut().keys_up.push(event.code());
        });
    });
    window.add_event_listener_with_callback("keyup", keyup.as_ref().unchecked_ref())?;
    keyup.forget();

    // Mouse click handler - writes to INPUT_BUFFER
    let canvas_width = canvas.width() as f64;
    let canvas_height = canvas.height() as f64;
    let click = Closure::<dyn FnMut(_)>::new(move |event: web_sys::MouseEvent| {
        // Get click position relative to canvas
        let target = event.target().unwrap();
        let canvas_el: &web_sys::HtmlCanvasElement = target.dyn_ref().unwrap();
        let rect = canvas_el.get_bounding_client_rect();
        let scale_x = canvas_width / rect.width();
        let scale_y = canvas_height / rect.height();
        let x = (event.client_x() as f64 - rect.left()) * scale_x;
        let y = (event.client_y() as f64 - rect.top()) * scale_y;

        INPUT_BUFFER.with(|buf| {
            buf.borrow_mut().click = Some((x, y));
        });
    });
    canvas.add_event_listener_with_callback("click", click.as_ref().unchecked_ref())?;
    click.forget();

    if let Some(doc) = window.document() {
        if let Some(input_el) = doc.get_element_by_id("mobile-input") {
            if let Ok(input_el) = input_el.dyn_into::<web_sys::HtmlInputElement>() {
                let input_listener = Closure::<dyn FnMut(_)>::new(move |event: web_sys::InputEvent| {
                    event.prevent_default();
                    let target = event.target().unwrap();
                    let input_el: web_sys::HtmlInputElement = target.dyn_into().unwrap();
                    let value = input_el.value();
                    if !value.is_empty() {
                        INPUT_BUFFER.with(|buf| {
                            let mut buf = buf.borrow_mut();
                            for c in value.chars() {
                                if c.is_ascii() && !c.is_ascii_control() {
                                    buf.chars.push(c);
                                }
                            }
                        });
                        input_el.set_value("");
                    }
                });
                input_el.add_event_listener_with_callback("input", input_listener.as_ref().unchecked_ref())?;
                input_listener.forget();

                let key_listener = Closure::<dyn FnMut(_)>::new(move |event: web_sys::KeyboardEvent| {
                    let handled = INPUT_BUFFER.with(|buf| {
                        let mut buf = buf.borrow_mut();
                        match event.code().as_str() {
                            "Backspace" => {
                                buf.backspace = true;
                                true
                            }
                            "Enter" => {
                                buf.enter = true;
                                true
                            }
                            "Escape" => {
                                buf.escape = true;
                                true
                            }
                            "Tab" => {
                                buf.tab = true;
                                true
                            }
                            _ => false,
                        }
                    });
                    if !handled && event.key() == "Enter" {
                        INPUT_BUFFER.with(|buf| {
                            buf.borrow_mut().enter = true;
                        });
                    }
                    if handled || event.key() == "Enter" {
                        event.prevent_default();
                        event.stop_propagation();
                    }
                });
                input_el.add_event_listener_with_callback("keydown", key_listener.as_ref().unchecked_ref())?;
                key_listener.forget();
            }
        }
    }

    // Touch handlers for mobile controls
    let touch_canvas = canvas.clone();
    let touch_state = Rc::clone(&state);
    let touchstart = Closure::<dyn FnMut(_)>::new(move |event: web_sys::TouchEvent| {
        event.prevent_default();
        let rect = touch_canvas.get_bounding_client_rect();
        let scale_x = canvas_width / rect.width();
        let scale_y = canvas_height / rect.height();
        let layout = touch_layout(canvas_width, canvas_height);

        let touches = event.changed_touches();
        for i in 0..touches.length() {
            if let Some(touch) = touches.item(i) {
                let x = (touch.client_x() as f64 - rect.left()) * scale_x;
                let y = (touch.client_y() as f64 - rect.top()) * scale_y;
                let pos = crate::math::Vec2::new(x as f32, y as f32);
                let id = touch.identifier();
                let mut handled_ui = false;
                let mut focus_input = false;
                let mut start_list_drag = false;

                {
                    let mut state_ref = touch_state.borrow_mut();
                    if state_ref.game.mobile_mode {
                        if state_ref.game.scene == Scene::Title {
                            let center_x = state_ref.game.width as f64 / 2.0;
                            let name_box_x = center_x - 90.0;
                            let name_box_y = 220.0 - 14.0;
                            let code_box_x = center_x - 90.0;
                            let code_box_y = 260.0 - 14.0;
                            let join_x = center_x + 20.0;
                            let join_y = 260.0 - 14.0;

                            if x >= name_box_x && x <= name_box_x + 180.0 && y >= name_box_y && y <= name_box_y + 22.0 {
                                state_ref.game.activate_text_input(0);
                                handled_ui = true;
                                focus_input = true;
                            } else if x >= code_box_x && x <= code_box_x + 100.0 && y >= code_box_y && y <= code_box_y + 22.0 {
                                state_ref.game.activate_text_input(1);
                                handled_ui = true;
                                focus_input = true;
                            } else if x >= join_x && x <= join_x + 70.0 && y >= join_y && y <= join_y + 22.0 {
                                state_ref.game.queued_join_room = true;
                                state_ref.game.text_input_active = false;
                                handled_ui = true;
                            }
                        } else if (state_ref.game.scene == Scene::Game || state_ref.game.scene == Scene::GameOver)
                            && state_ref.game.map_open
                        {
                            let map_left = crate::game::MAP_OVERLAY_PADDING as f64;
                            let map_top = crate::game::MAP_OVERLAY_PADDING as f64;
                            let map_size = crate::game::MAP_OVERLAY_SIZE as f64;
                            if x < map_left || x > map_left + map_size || y < map_top || y > map_top + map_size {
                                state_ref.game.map_open = false;
                                handled_ui = true;
                            } else {
                                let input_x = map_left + 10.0;
                                let input_y = map_top + map_size - 6.0;
                                let button_w = 100.0;
                                let button_h = 20.0;
                                let button_x = map_left + map_size - button_w - 10.0;
                                let button_y = input_y - 28.0;
                                if x >= button_x && x <= button_x + button_w && y >= button_y && y <= button_y + button_h {
                                    state_ref.game.confirm_map_teleport();
                                    handled_ui = true;
                                } else if x >= map_left && x <= map_left + map_size && y >= map_top && y <= map_top + map_size {
                                    state_ref.game.handle_map_click(x, y);
                                    handled_ui = true;
                                } else if x >= input_x && x <= input_x + 140.0 && y >= input_y - 14.0 && y <= input_y + 8.0 {
                                    state_ref.game.activate_map_input(0);
                                    handled_ui = true;
                                    focus_input = true;
                                } else if x >= input_x + 170.0 && x <= input_x + 310.0 && y >= input_y - 14.0 && y <= input_y + 8.0 {
                                    state_ref.game.activate_map_input(1);
                                    handled_ui = true;
                                    focus_input = true;
                                }
                            }
                        } else if state_ref.game.mobile_mode
                            && (state_ref.game.scene == Scene::Game || state_ref.game.scene == Scene::GameOver)
                        {
                            let in_circle = |center: crate::math::Vec2, radius: f64| {
                                let dx = pos.x as f64 - center.x as f64;
                                let dy = pos.y as f64 - center.y as f64;
                                dx * dx + dy * dy <= radius * radius
                            };
                            if in_circle(layout.chat_center, layout.top_button_radius) {
                                state_ref.game.toggle_chat();
                                handled_ui = true;
                                focus_input = state_ref.game.chat_open;
                            } else if !state_ref.game.map_open && !state_ref.game.player_list_open {
                            let map_size = 120.0;
                            let map_padding = 10.0;
                            let map_left = (state_ref.game.width as f64) - map_size - map_padding;
                            let map_top = if state_ref.game.mobile_mode { 130.0 } else { (state_ref.game.height as f64) - map_size - map_padding };
                            if x >= map_left && x <= map_left + map_size && y >= map_top && y <= map_top + map_size {
                                state_ref.game.toggle_map();
                                handled_ui = true;
                            } else {
                                let right = state_ref.game.width as f64 - 10.0;
                                let left = right - 240.0;
                                let top = 35.0;
                                let bottom = 110.0;
                                if x >= left && x <= right && y >= top && y <= bottom {
                                    state_ref.game.toggle_player_list();
                                    handled_ui = true;
                                }
                            }
                            } else if state_ref.game.player_list_open {
                                let overlay_w = 480.0;
                                let _overlay_h = 360.0;
                                let left = (state_ref.game.width as f64 - overlay_w) / 2.0;
                                let top = 70.0;
                                let close_x = left + overlay_w - 26.0;
                                let close_y = top + 8.0;
                                if x >= close_x && x <= close_x + 18.0 && y >= close_y && y <= close_y + 18.0 {
                                    state_ref.game.toggle_player_list();
                                    handled_ui = true;
                                } else if x >= left && x <= left + overlay_w && y >= top && y <= top + 360.0 {
                                    handled_ui = true;
                                    start_list_drag = true;
                                }
                            }
                        }
                    }
                }

                if focus_input {
                    MOBILE_INPUT.with(|cell| {
                        if let Some(input_el) = cell.borrow().as_ref() {
                            let _ = input_el.focus();
                        }
                    });
                }
                if handled_ui {
                    if start_list_drag {
                        TOUCH_STATE.with(|state| {
                            let mut state = state.borrow_mut();
                            state.list_drag_id = Some(id);
                            state.list_drag_last = pos;
                            state.list_scroll_delta = 0.0;
                        });
                    }
                    continue;
                }

                TOUCH_STATE.with(|state| {
                    let mut state = state.borrow_mut();
                    let in_circle = |center: crate::math::Vec2, radius: f64| {
                        let dx = pos.x as f64 - center.x as f64;
                        let dy = pos.y as f64 - center.y as f64;
                        dx * dx + dy * dy <= radius * radius
                    };

                    if state.joystick_id.is_none() && in_circle(layout.stick_center, layout.stick_radius * 1.3) {
                        state.joystick_id = Some(id);
                        state.joystick_center = pos;
                        state.joystick_axis = crate::math::Vec2::ZERO;
                        return;
                    }
                    if state.attack_id.is_none() && in_circle(layout.attack_center, layout.button_radius) {
                        state.attack_id = Some(id);
                        return;
                    }
                    if state.phase_id.is_none() && in_circle(layout.phase_center, layout.button_radius) {
                        state.phase_id = Some(id);
                        return;
                    }
                    if in_circle(layout.map_center, layout.top_button_radius) {
                        state.map_tap_frames = 2;
                        return;
                    }
                    if in_circle(layout.list_center, layout.top_button_radius) {
                        state.list_tap_frames = 2;
                        return;
                    }
                    if in_circle(layout.chat_center, layout.top_button_radius) {
                        state.chat_tap_frames = 2;
                        return;
                    }
                    if in_circle(layout.zoom_in_center, layout.top_button_radius) {
                        state.zoom_in_tap_frames = 2;
                        return;
                    }
                    if in_circle(layout.zoom_out_center, layout.top_button_radius) {
                        state.zoom_out_tap_frames = 2;
                        return;
                    }

                    let map_left = crate::game::MAP_OVERLAY_PADDING as f64;
                    let map_top = crate::game::MAP_OVERLAY_PADDING as f64;
                    let map_size = crate::game::MAP_OVERLAY_SIZE as f64;
                    if x >= map_left && x <= map_left + map_size && y >= map_top && y <= map_top + map_size {
                        state.map_drag_id = Some(id);
                        state.map_drag_last = pos;
                        state.map_drag_distance = 0.0;
                        state.map_tap_candidate = true;
                        state.map_tap_pos = pos;
                    }
                });
            }
        }
    });
    canvas.add_event_listener_with_callback("touchstart", touchstart.as_ref().unchecked_ref())?;
    touchstart.forget();

    let touch_canvas = canvas.clone();
    let touchmove = Closure::<dyn FnMut(_)>::new(move |event: web_sys::TouchEvent| {
        event.prevent_default();
        let rect = touch_canvas.get_bounding_client_rect();
        let scale_x = canvas_width / rect.width();
        let scale_y = canvas_height / rect.height();
        let layout = touch_layout(canvas_width, canvas_height);

        let touches = event.changed_touches();
        for i in 0..touches.length() {
            if let Some(touch) = touches.item(i) {
                let x = (touch.client_x() as f64 - rect.left()) * scale_x;
                let y = (touch.client_y() as f64 - rect.top()) * scale_y;
                let pos = crate::math::Vec2::new(x as f32, y as f32);
                let id = touch.identifier();

                TOUCH_STATE.with(|state| {
                    let mut state = state.borrow_mut();
                    if state.joystick_id == Some(id) {
                        let delta = pos - state.joystick_center;
                        let mut axis = delta;
                        let max = layout.stick_radius as f32;
                        let len = axis.length();
                        if len > max && len > 0.0 {
                            axis = axis * (max / len);
                        }
                        state.joystick_axis = if max > 0.0 { axis / max } else { crate::math::Vec2::ZERO };
                    }
                    if state.map_drag_id == Some(id) {
                        let delta = pos - state.map_drag_last;
                        state.map_drag_delta = delta;
                        state.map_drag_distance += delta.length();
                        state.map_drag_last = pos;
                    }
                    if state.list_drag_id == Some(id) {
                        let delta = pos.y - state.list_drag_last.y;
                        state.list_scroll_delta += delta;
                        state.list_drag_last = pos;
                    }
                });
            }
        }

        let all_touches = event.touches();
        if all_touches.length() >= 2 {
            if let (Some(t1), Some(t2)) = (all_touches.item(0), all_touches.item(1)) {
                let x1 = (t1.client_x() as f64 - rect.left()) * scale_x;
                let y1 = (t1.client_y() as f64 - rect.top()) * scale_y;
                let x2 = (t2.client_x() as f64 - rect.left()) * scale_x;
                let y2 = (t2.client_y() as f64 - rect.top()) * scale_y;
                let p1 = crate::math::Vec2::new(x1 as f32, y1 as f32);
                let p2 = crate::math::Vec2::new(x2 as f32, y2 as f32);
                let dist = (p1 - p2).length();
                let id1 = t1.identifier();
                let id2 = t2.identifier();

                TOUCH_STATE.with(|state| {
                    let mut state = state.borrow_mut();
                    if state.pinch_ids.is_none() {
                        state.pinch_ids = Some((id1, id2));
                        state.pinch_start_distance = dist.max(1.0);
                    } else if let Some((a, b)) = state.pinch_ids {
                        if (a == id1 && b == id2) || (a == id2 && b == id1) {
                            let delta = dist - state.pinch_start_distance;
                            state.map_drag_delta = crate::math::Vec2::new(0.0, delta);
                        }
                    }
                });
            }
        }
    });
    canvas.add_event_listener_with_callback("touchmove", touchmove.as_ref().unchecked_ref())?;
    touchmove.forget();

    let touchend = Closure::<dyn FnMut(_)>::new(move |event: web_sys::TouchEvent| {
        event.prevent_default();
        let touches = event.changed_touches();
        for i in 0..touches.length() {
            if let Some(touch) = touches.item(i) {
                let id = touch.identifier();
                TOUCH_STATE.with(|state| {
                    let mut state = state.borrow_mut();
                    if state.joystick_id == Some(id) {
                        state.joystick_id = None;
                        state.joystick_axis = crate::math::Vec2::ZERO;
                    }
                    if state.attack_id == Some(id) {
                        state.attack_id = None;
                    }
                    if state.phase_id == Some(id) {
                        state.phase_id = None;
                    }
                    if state.map_drag_id == Some(id) {
                        state.map_drag_id = None;
                        state.map_drag_delta = crate::math::Vec2::ZERO;
                        state.map_drag_distance = 0.0;
                        state.map_tap_candidate = false;
                    }
                    if state.list_drag_id == Some(id) {
                        state.list_drag_id = None;
                        state.list_scroll_delta = 0.0;
                    }
                    if let Some((a, b)) = state.pinch_ids {
                        if a == id || b == id {
                            state.pinch_ids = None;
                            state.pinch_start_distance = 0.0;
                        }
                    }
                });
            }
        }
    });
    canvas.add_event_listener_with_callback("touchend", touchend.as_ref().unchecked_ref())?;
    canvas.add_event_listener_with_callback("touchcancel", touchend.as_ref().unchecked_ref())?;
    touchend.forget();

    Ok(())
}

fn start_game_loop(window: web_sys::Window, state: Rc<RefCell<GameState>>) -> Result<(), JsValue> {
    let f: Rc<RefCell<Option<Closure<dyn FnMut()>>>> = Rc::new(RefCell::new(None));
    let g = f.clone();

    let window_clone = window.clone();
    *g.borrow_mut() = Some(Closure::new(move || {
        {
            let mut state_ref = state.borrow_mut();

            // Process input buffer - transfer events from buffer to game state
            INPUT_BUFFER.with(|buf| {
                let mut buf = buf.borrow_mut();

                // On title screen, handle Tab to cycle between name → room code → menu
                if state_ref.game.scene == Scene::Title && buf.tab {
                    if state_ref.game.text_input_active {
                        if state_ref.game.active_input_field == 0 {
                            // Name field → Room code field
                            state_ref.game.active_input_field = 1;
                        } else {
                            // Room code field → Menu (deactivate text input)
                            state_ref.game.text_input_active = false;
                        }
                    } else {
                        // Menu → Name field
                        state_ref.game.activate_text_input(0);
                    }
                }

                if (state_ref.game.scene == Scene::Game || state_ref.game.scene == Scene::GameOver)
                    && state_ref.game.map_open
                    && buf.tab
                {
                    let next = if state_ref.game.map_active_field == 0 { 1 } else { 0 };
                    state_ref.game.activate_map_input(next);
                }

                // Handle text input mode
                if state_ref.game.is_text_input_active() {
                    // Process character input
                    for c in buf.chars.drain(..) {
                        state_ref.game.handle_char_input(c);
                    }

                    // Handle backspace
                    if buf.backspace {
                        state_ref.game.handle_backspace();
                    }

                    if state_ref.game.active_input_field == 0 {
                        save_player_name(&state_ref.game.player_name);
                    } else if state_ref.game.active_input_field == 1 {
                        save_room_code(&state_ref.game.room_code_input);
                    }

                    // Handle escape - exit text input to menu
                    if buf.escape {
                        state_ref.game.text_input_active = false;
                    }

                    // Handle enter - confirm and exit text input to menu
                    if buf.enter {
                        if state_ref.game.active_input_field == 0 {
                            save_player_name(&state_ref.game.player_name);
                        } else if state_ref.game.active_input_field == 1 {
                            save_room_code(&state_ref.game.room_code_input);
                            state_ref.game.queued_join_room = true;
                        }
                        state_ref.game.text_input_active = false;
                    }

                    // Clear keys since we're in text mode (don't pass to game input)
                    buf.keys_down.clear();
                    buf.keys_up.clear();
                } else if state_ref.game.is_map_input_active() {
                    for c in buf.chars.drain(..) {
                        state_ref.game.handle_map_char_input(c);
                    }

                    if buf.backspace {
                        state_ref.game.handle_map_backspace();
                    }

                    if buf.escape {
                        state_ref.game.map_text_input_active = false;
                    }

                    if buf.enter {
                        state_ref.game.confirm_map_teleport_from_inputs();
                        state_ref.game.map_text_input_active = false;
                    }

                    buf.keys_down.clear();
                    buf.keys_up.clear();
                } else if state_ref.game.is_chat_input_active() {
                    for c in buf.chars.drain(..) {
                        state_ref.game.handle_chat_char_input(c);
                    }

                    if buf.backspace {
                        state_ref.game.handle_chat_backspace();
                    }

                    if buf.escape {
                        state_ref.game.close_chat();
                    }

                    if buf.enter {
                        if let Some(text) = state_ref.game.take_chat_input() {
                            if state_ref.game.can_send_chat() {
                                let local_hash = state_ref.network.local_peer_hash.unwrap_or(0);
                                let trimmed = text.trim();
                                if trimmed.to_ascii_lowercase().starts_with("/mute") {
                                    let target = trimmed.splitn(2, ' ').nth(1).unwrap_or("").trim();
                                    if local_hash == 0 {
                                        state_ref.game.push_chat_line(
                                            "System".to_string(),
                                            "Chat is still initializing.".to_string(),
                                        );
                                    } else if target.is_empty() {
                                        state_ref.game.push_chat_line(
                                            "System".to_string(),
                                            "Usage: /mute NAME".to_string(),
                                        );
                                    } else if let Some(target_hash) = state_ref.network.resolve_hash_by_name(target) {
                                        if target_hash == local_hash {
                                            state_ref.game.push_chat_line(
                                                "System".to_string(),
                                                "You cannot mute yourself.".to_string(),
                                            );
                                        } else {
                                            state_ref.network.mute_locally(target_hash);
                                            let vote = net::VoteMute {
                                                target_hash,
                                                voter_hash: local_hash,
                                            };
                                            let muted_now = state_ref.network.register_vote_mute(vote);
                                            state_ref.network.send_vote_mute(vote);
                                            let target_name = state_ref.network.display_name_for_hash(target_hash);
                                            let msg = if muted_now {
                                                format!("Muted {}.", target_name)
                                            } else {
                                                format!("Muted {} locally. Vote sent.", target_name)
                                            };
                                            state_ref.game.push_chat_line("System".to_string(), msg);
                                        }
                                    } else {
                                        state_ref.game.push_chat_line(
                                            "System".to_string(),
                                            format!("No player named '{}'.", target),
                                        );
                                    }
                                } else if local_hash == 0 && !state_ref.network.room_code.is_empty() {
                                    state_ref.game.push_chat_line(
                                        "System".to_string(),
                                        "Chat is still initializing.".to_string(),
                                    );
                                } else {
                                    let name = state_ref.game.player_name.clone();
                                    state_ref.game.push_chat_line(name.clone(), trimmed.to_string());
                                    if !state_ref.network.room_code.is_empty() {
                                        state_ref.network.send_chat_message(net::ChatMessage {
                                            sender_hash: local_hash,
                                            text: trimmed.to_string(),
                                        });
                                    }
                                }
                                state_ref.game.mark_chat_sent();
                            } else {
                                state_ref.game.push_chat_line(
                                    "System".to_string(),
                                    "Slow down. Chat has a short cooldown.".to_string(),
                                );
                            }
                        }
                        state_ref.game.close_chat();
                    }

                    buf.keys_down.clear();
                    buf.keys_up.clear();
                } else {
                    if state_ref.game.player_list_open
                        && (state_ref.game.scene == Scene::Game || state_ref.game.scene == Scene::GameOver)
                    {
                        let blocked = ["KeyZ", "KeyX", "ArrowLeft", "ArrowRight"];
                        buf.keys_down.retain(|code| !blocked.contains(&code.as_str()));
                        buf.keys_up.retain(|code| !blocked.contains(&code.as_str()));
                    }
                    if state_ref.game.map_open
                        && (state_ref.game.scene == Scene::Game || state_ref.game.scene == Scene::GameOver)
                    {
                        let allowed = [
                            "KeyW", "KeyA", "KeyS", "KeyD",
                            "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight",
                            "KeyZ", "Space", "KeyX", "ShiftLeft", "ShiftRight",
                            "KeyM",
                        ];
                        buf.keys_down.retain(|code| allowed.contains(&code.as_str()));
                        buf.keys_up.retain(|code| allowed.contains(&code.as_str()));
                    }
                    if (state_ref.game.scene == Scene::Game || state_ref.game.scene == Scene::GameOver)
                        && !state_ref.game.map_open
                    {
                        let mut handled_keys: Vec<String> = Vec::new();
                        for code in &buf.keys_down {
                            match code.as_str() {
                                "KeyP" => {
                                    state_ref.game.toggle_player_list();
                                    handled_keys.push(code.clone());
                                }
                                "ArrowDown" => {
                                    if state_ref.game.player_list_open && !state_ref.game.player_list_search_active {
                                        state_ref.game.scroll_player_list(1);
                                        handled_keys.push(code.clone());
                                    }
                                }
                                "ArrowUp" => {
                                    if state_ref.game.player_list_open && !state_ref.game.player_list_search_active {
                                        state_ref.game.scroll_player_list(-1);
                                        handled_keys.push(code.clone());
                                    }
                                }
                                "KeyS" => {
                                    if state_ref.game.player_list_open {
                                        state_ref.game.cycle_player_list_sort();
                                        handled_keys.push(code.clone());
                                    }
                                }
                                "KeyD" => {
                                    if state_ref.game.player_list_open {
                                        state_ref.game.toggle_player_list_sort_order();
                                        handled_keys.push(code.clone());
                                    }
                                }
                                "KeyC" => {
                                    if state_ref.game.scene == Scene::Game {
                                        state_ref.game.toggle_chat();
                                        handled_keys.push(code.clone());
                                    }
                                }
                                "Slash" => {
                                    if state_ref.game.player_list_open {
                                        state_ref.game.activate_player_list_search();
                                        handled_keys.push(code.clone());
                                    }
                                }
                                "Escape" => {
                                    if state_ref.game.player_list_open {
                                        state_ref.game.clear_player_list_search();
                                        handled_keys.push(code.clone());
                                    }
                                }
                                _ => {}
                            }
                        }
                        if !handled_keys.is_empty() {
                            buf.keys_down.retain(|code| !handled_keys.contains(code));
                        }
                    }

                    if state_ref.game.player_list_open && state_ref.game.player_list_search_active {
                        for c in buf.chars.drain(..) {
                            state_ref.game.handle_player_list_char_input(c);
                        }
                        if buf.backspace {
                            state_ref.game.handle_player_list_backspace();
                        }
                        if buf.escape {
                            state_ref.game.clear_player_list_search();
                        }
                        buf.keys_down.clear();
                        buf.keys_up.clear();
                    }

                    if (state_ref.game.scene == Scene::Game || state_ref.game.scene == Scene::GameOver)
                        && state_ref.game.map_open
                        && buf.chars.iter().any(|c| c.is_ascii_digit() || *c == '-' || *c == '.')
                    {
                        state_ref.game.activate_map_input(0);
                        for c in buf.chars.drain(..) {
                            state_ref.game.handle_map_char_input(c);
                        }
                        buf.keys_down.clear();
                        buf.keys_up.clear();
                        buf.clear();
                        return;
                    }
                    // Process key down events for game input
                    for code in buf.keys_down.drain(..) {
                        state_ref.input.key_down(&code);
                    }

                    // Process key up events
                    for code in buf.keys_up.drain(..) {
                        state_ref.input.key_up(&code);
                    }
                }

                // Handle click for text input field selection on title screen
                if let Some((x, y)) = buf.click {
                    if state_ref.game.scene == Scene::Title {
                        let center_x = state_ref.game.width as f64 / 2.0;

                        // Check if clicked on name input
                        let name_box_x = center_x - 90.0;
                        let name_box_y = 220.0 - 14.0;
                        if x >= name_box_x && x <= name_box_x + 180.0 && y >= name_box_y && y <= name_box_y + 22.0 {
                            state_ref.game.activate_text_input(0);
                        }
                        // Check if clicked on room code input
                        else {
                            let code_box_x = center_x - 90.0;
                            let code_box_y = 260.0 - 14.0;
                            if x >= code_box_x && x <= code_box_x + 100.0 && y >= code_box_y && y <= code_box_y + 22.0 {
                                state_ref.game.activate_text_input(1);
                            } else {
                                let join_x = center_x + 20.0;
                                let join_y = 260.0 - 14.0;
                                if x >= join_x && x <= join_x + 70.0 && y >= join_y && y <= join_y + 22.0 {
                                    state_ref.game.queued_join_room = true;
                                    state_ref.game.text_input_active = false;
                                } else {
                                    // Clicked elsewhere, deactivate text input
                                    state_ref.game.text_input_active = false;
                                }
                            }
                        }
                    } else if state_ref.game.scene == Scene::Game || state_ref.game.scene == Scene::GameOver {
                        if state_ref.game.map_open {
                            let map_left = crate::game::MAP_OVERLAY_PADDING as f64;
                            let map_top = crate::game::MAP_OVERLAY_PADDING as f64;
                            let map_size = crate::game::MAP_OVERLAY_SIZE as f64;
                            let close_x = map_left + map_size - 26.0;
                            let close_y = map_top + 8.0;
                            if x >= close_x && x <= close_x + 18.0 && y >= close_y && y <= close_y + 18.0 {
                                state_ref.game.map_open = false;
                                return;
                            }
                            let input_x = map_left + 10.0;
                            let input_y = map_top + map_size - 6.0;
                            let button_w = 100.0;
                            let button_h = 20.0;
                            let button_x = map_left + map_size - button_w - 10.0;
                            let button_y = input_y - 28.0;
                            if x >= button_x && x <= button_x + button_w && y >= button_y && y <= button_y + button_h {
                                state_ref.game.confirm_map_teleport();
                                return;
                            }
                            if x >= input_x && x <= input_x + 140.0 && y >= input_y - 14.0 && y <= input_y + 8.0 {
                                state_ref.game.activate_map_input(0);
                            } else if x >= input_x + 170.0 && x <= input_x + 310.0 && y >= input_y - 14.0 && y <= input_y + 8.0 {
                                state_ref.game.activate_map_input(1);
                            } else {
                                state_ref.game.handle_map_click(x, y);
                            }
                        } else if state_ref.game.player_list_open {
                            let overlay_w = 480.0;
                            let overlay_h = 360.0;
                            let left = (state_ref.game.width as f64 - overlay_w) / 2.0;
                            let top = 70.0;
                            if !(x >= left && x <= left + overlay_w && y >= top && y <= top + overlay_h) {
                                state_ref.game.toggle_player_list();
                            }
                        } else {
                            let map_size = 120.0;
                            let map_padding = 10.0;
                            let map_left = (state_ref.game.width as f64) - map_size - map_padding;
                            let map_top = if state_ref.game.mobile_mode { 130.0 } else { (state_ref.game.height as f64) - map_size - map_padding };
                            if x >= map_left && x <= map_left + map_size && y >= map_top && y <= map_top + map_size {
                                state_ref.game.toggle_map();
                            } else {
                                let right = state_ref.game.width as f64 - 10.0;
                                let left = right - 240.0;
                                let top = 35.0;
                                let bottom = 110.0;
                                if x >= left && x <= right && y >= top && y <= bottom {
                                    state_ref.game.toggle_player_list();
                                }
                            }
                        }
                    }
                }

                if buf.enter
                    && (state_ref.game.scene == Scene::Game || state_ref.game.scene == Scene::GameOver)
                    && state_ref.game.map_open
                {
                    if !state_ref.game.map_text_input_active {
                        state_ref.game.confirm_map_teleport_from_inputs();
                    }
                }

                // Clear the buffer
                buf.clear();
            });

            if state_ref.game.mobile_mode {
                TOUCH_STATE.with(|state| {
                    let mut state = state.borrow_mut();
                    let mut down_mask = 0u8;
                    let mut axis = None;
                    if !state_ref.game.map_open && !state_ref.game.player_list_open {
                        if state.attack_id.is_some() {
                            down_mask |= crate::input::BUTTON_ATTACK;
                        }
                        if state.phase_id.is_some() {
                            down_mask |= crate::input::BUTTON_PHASE;
                        }
                        if state.joystick_id.is_some() {
                            axis = Some(state.joystick_axis);
                        }
                    }

                    state_ref.input.set_touch_state(axis, down_mask);

                    if state.map_tap_frames > 0 {
                        if state_ref.game.scene == Scene::Game || state_ref.game.scene == Scene::GameOver {
                            state_ref.game.toggle_map();
                        }
                        state.map_tap_frames = 0;
                    }
                    if state.list_tap_frames > 0 {
                        state_ref.game.toggle_player_list();
                        state.list_tap_frames = 0;
                    }
                    if state.chat_tap_frames > 0 {
                        state_ref.game.toggle_chat();
                        state.chat_tap_frames = 0;
                        if state_ref.game.mobile_mode && state_ref.game.chat_open {
                            MOBILE_INPUT.with(|cell| {
                                if let Some(input_el) = cell.borrow().as_ref() {
                                    let _ = input_el.focus();
                                }
                            });
                        }
                    }

                    if state_ref.game.map_open {
                        if state.zoom_in_tap_frames > 0 {
                            state_ref.game.zoom_map_in();
                            state.zoom_in_tap_frames = 0;
                        }
                        if state.zoom_out_tap_frames > 0 {
                            state_ref.game.zoom_map_out();
                            state.zoom_out_tap_frames = 0;
                        }
                        if state.pinch_ids.is_some() && state.map_drag_delta.y.abs() > 0.0 {
                            if state.map_drag_delta.y > 0.0 {
                                state_ref.game.zoom_map_in();
                            } else {
                                state_ref.game.zoom_map_out();
                            }
                            state.map_drag_delta = crate::math::Vec2::ZERO;
                        }
                        if state.map_drag_delta.x != 0.0 || state.map_drag_delta.y != 0.0 {
                            let pan_dx = if state_ref.game.mobile_mode { -state.map_drag_delta.x } else { state.map_drag_delta.x };
                            let pan_dy = if state_ref.game.mobile_mode { -state.map_drag_delta.y } else { state.map_drag_delta.y };
                            state_ref.game.pan_map_by_screen_delta(
                                pan_dx,
                                pan_dy,
                            );
                            state.map_drag_delta = crate::math::Vec2::ZERO;
                        }
                        if state.map_tap_candidate && state.map_drag_distance < 8.0 {
                            state_ref.game.handle_map_click(state.map_tap_pos.x as f64, state.map_tap_pos.y as f64);
                            state.map_tap_candidate = false;
                        }
                    } else if state_ref.game.player_list_open {
                        if state.list_scroll_delta.abs() >= 1.0 {
                            let steps = (-state.list_scroll_delta / 18.0).round() as i32;
                            if steps != 0 {
                                state_ref.game.scroll_player_list(steps);
                                state.list_scroll_delta = 0.0;
                            }
                        }
                    } else {
                        state.map_drag_delta = crate::math::Vec2::ZERO;
                        state.list_scroll_delta = 0.0;
                    }
                });
            } else {
                state_ref.input.set_touch_state(None, 0);
            }

            MOBILE_INPUT.with(|cell| {
                if let Some(input_el) = cell.borrow().as_ref() {
                    let wants_text = state_ref.game.scene == Scene::Title && state_ref.game.text_input_active
                        || state_ref.game.is_chat_input_active()
                        || state_ref.game.is_map_input_active();
                    if wants_text {
                        let _ = input_el.focus();
                    } else {
                        let _ = input_el.blur();
                    }
                }
            });

            // Clone input to avoid borrow conflict
            let input_snapshot = state_ref.input.clone();

            // Process debug commands (from JS/console)
            let commands: Vec<String> = DEBUG_COMMANDS.with(|queue| queue.borrow_mut().drain(..).collect());
            for command in commands {
                match state_ref.game.apply_debug_command(&command) {
                    Ok(message) => web_sys::console::log_1(&format!("[debug] {}", message).into()),
                    Err(err) => web_sys::console::warn_1(&format!("[debug] {} -> {}", command, err).into()),
                }
            }

            // Check for menu actions before updating game
            let (create_room, join_room, room_code) = state_ref.game.get_menu_action(&input_snapshot);

            // Handle network actions
            if create_room {
                // Sync player name to network before creating room
                let player_name = state_ref.game.player_name.clone();
                state_ref.network.set_player_name(&player_name);
                let server = signaling_server_url();
                let code = state_ref.network.create_room(&server);
                // Store the room code so it displays in the UI
                state_ref.game.room_code_input = code;
                // Don't start game yet - wait for connection on title screen
            } else if join_room {
                // Sync player name to network before joining room
                let player_name = state_ref.game.player_name.clone();
                state_ref.network.set_player_name(&player_name);
                let server = signaling_server_url();
                state_ref.network.join_room(&server, &room_code);
                // Don't start game yet - wait for connection on title screen
            }

            // Update network (safely - returns false on connection failure)
            let frame_count = state_ref.game.frame_count;
            let network_ok = state_ref.network.update(frame_count);
            let supernode_id = state_ref.network.supernode_id;
            let became_host = state_ref.network.is_host
                && state_ref.last_supernode_id != supernode_id;
            state_ref.last_supernode_id = supernode_id;

            // Check if we should transition to game based on network state
            if state_ref.game.scene == Scene::Title {
                match state_ref.network.state {
                    NetworkState::Connected | NetworkState::WaitingForPeers => {
                        // In multiplayer, respawn instead of going to title
                        if !state_ref.network.room_code.is_empty() {
                            // Check if this was a respawn (we already have a wave/enemies)
                            if state_ref.game.wave > 0 {
                                // Respawn - keep enemies and wave
                                state_ref.game.respawn_in_multiplayer();
                            } else {
                                // Fresh start
                                state_ref.game.start_game_with_network();
                            }
                        }
                    }
                    NetworkState::Error(_) => {
                        // Connection failed, clear room code to allow retry
                        // (keep on title screen)
                    }
                    _ => {
                        // Still connecting
                    }
                }
            }

            // If network failed during game, return to title
            if !network_ok && state_ref.game.scene == Scene::Game {
                state_ref.game.scene = Scene::Title;
            }

            // Update game - use multiplayer version when in a room (Connected or WaitingForPeers)
            let in_multiplayer_room = !state_ref.network.room_code.is_empty() &&
                (state_ref.network.state == NetworkState::Connected ||
                 state_ref.network.state == NetworkState::WaitingForPeers);

            if in_multiplayer_room && state_ref.game.scene == Scene::Game {
                let incoming_inputs = state_ref.network.take_input_frames();
                if !incoming_inputs.is_empty() {
                    let mut formatted_inputs = Vec::with_capacity(incoming_inputs.len());
                    for (peer_id, input_frame) in incoming_inputs {
                        formatted_inputs.push((format!("{:?}", peer_id), input_frame));
                    }
                    state_ref.game.queue_remote_inputs(&formatted_inputs);
                }
                let remote_players = state_ref.network.remote_players.clone();
                state_ref.game.update_remote_predictions(&remote_players);
                let predictions = state_ref.game.remote_predictions().clone();
                state_ref.network.apply_predicted_states(&predictions);
            }

            let prev_scene = state_ref.game.scene;
            if in_multiplayer_room {
                // Clone remote players to avoid borrow conflict
                let remote_players = state_ref.network.remote_players.clone();
                let is_host = state_ref.network.is_host;
                state_ref.game.update_multiplayer(
                    &input_snapshot,
                    &remote_players,
                    is_host,
                );
            } else {
                state_ref.game.update(&input_snapshot);
            }
            state_ref.input.end_frame();

            if prev_scene != Scene::Game && state_ref.game.scene == Scene::Game && in_multiplayer_room {
                state_ref.network.reset_stats();
            }

            if in_multiplayer_room && state_ref.game.scene == Scene::Game {
                state_ref.network.tick_playtime(true);
            }

            if in_multiplayer_room && state_ref.game.scene == Scene::Game && became_host {
                if let Some(wave_start) = state_ref.game.last_wave_start {
                    state_ref.network.send_wave_start(wave_start);
                }
                let enemy_sync = state_ref.game.create_enemy_sync();
                state_ref.network.send_enemy_sync(enemy_sync);
                let paid_obstacles = state_ref.game.paid_obstacles.clone();
                state_ref.network.send_paid_obstacles_to_all(&paid_obstacles);
            }

            // Multiplayer sync logic (only during gameplay with network)
            if state_ref.game.scene == Scene::Game &&
               (state_ref.network.state == NetworkState::Connected || state_ref.network.state == NetworkState::WaitingForPeers) {

                // Wave start sync - broadcast wave spawns so all clients spawn identically
                if state_ref.network.is_host {
                    // Host: Broadcast wave start when a new wave spawns
                    if let Some(wave_start) = state_ref.game.take_pending_wave_start() {
                        state_ref.network.send_wave_start(wave_start);
                    }

                    // Host: Send current wave state AND enemy sync to late joiners
                    if state_ref.network.has_new_peers_needing_state() {
                        let new_peers = state_ref.network.take_new_peers_needing_state();
                        if let Some(wave_start) = state_ref.game.last_wave_start {
                            state_ref.network.send_wave_start_to_peers(&wave_start, &new_peers);
                        }
                        // Also send enemy sync so late joiners see current enemy state (alive/dead)
                        let enemy_sync = state_ref.game.create_enemy_sync();
                        state_ref.network.send_enemy_sync(enemy_sync);
                        let paid_obstacles = state_ref.game.paid_obstacles.clone();
                        state_ref.network.send_paid_obstacles_to_peers(&paid_obstacles, &new_peers);
                    }
                } else {
                    // Client: Apply wave start from host to spawn enemies deterministically
                    if let Some(wave_start) = state_ref.network.take_wave_start() {
                        state_ref.game.apply_wave_start(&wave_start);
                    }
                }

                // Enemy kill events - broadcast kills so all clients see the same deaths
                // All players broadcast their kills (authoritative from killer's machine)
                let kills = state_ref.game.take_pending_kills();
                let player_pos = state_ref.game.player.pos;
                for (enemy_type, enemy_id) in kills {
                    state_ref.network.record_local_kill(enemy_type);
                    let killer_hash = state_ref.network.local_peer_hash.unwrap_or(0);
                    state_ref.network.send_enemy_kill(net::EnemyKill {
                        enemy_type: enemy_type as u8,
                        enemy_id,
                        killer_x: player_pos.x,
                        killer_y: player_pos.y,
                        killer_hash,
                    });
                }

                // Process enemy kills from other players
                let remote_kills = state_ref.network.take_enemy_kills();
                for (_peer_id, kill) in remote_kills {
                    if let Some(enemy_type) = net::EnemyType::from_u8(kill.enemy_type) {
                        state_ref.game.kill_enemy(enemy_type, kill.enemy_id);
                        if let Some(local_hash) = state_ref.network.local_peer_hash {
                            if kill.killer_hash == local_hash {
                                continue;
                            }
                        }
                        if let Some(remote_id) = state_ref.network.resolve_peer_hash(kill.killer_hash) {
                            state_ref.network.record_remote_kill(&remote_id, enemy_type);
                        }
                    }
                }

                // Player death events - broadcast deaths so stats stay in sync
                let deaths = state_ref.game.take_pending_deaths();
                if !deaths.is_empty() {
                    state_ref.network.record_local_deaths(deaths.len() as u32);
                }
                for mut death in deaths {
                    let victim_hash = state_ref.network.local_peer_hash.unwrap_or(0);
                    death.victim_hash = victim_hash;
                    state_ref.network.send_player_death(death);
                }

                // Process player deaths from other players
                let remote_deaths = state_ref.network.take_player_deaths();
                for (_peer_id, death) in remote_deaths {
                    if let Some(local_hash) = state_ref.network.local_peer_hash {
                        if death.victim_hash == local_hash {
                            continue;
                        }
                    }
                    if let Some(remote_id) = state_ref.network.resolve_peer_hash(death.victim_hash) {
                        state_ref.network.record_remote_death(&remote_id, 1);
                    }
                }

                let incoming_chat = state_ref.network.take_chat_messages();
                for chat in incoming_chat {
                    let name = state_ref.network.display_name_for_hash(chat.sender_hash);
                    state_ref.game.push_chat_line(name, chat.text);
                }

                let incoming_mutes = state_ref.network.take_vote_mutes();
                for vote in incoming_mutes {
                    if let Some(local_hash) = state_ref.network.local_peer_hash {
                        if vote.target_hash == local_hash {
                            continue;
                        }
                    }
                    let target_name = state_ref.network.display_name_for_hash(vote.target_hash);
                    state_ref.game
                        .push_chat_line("System".to_string(), format!("Muted {}.", target_name));
                }

                // Paid obstacle events - broadcast and apply with verification
                let paid_obstacles = state_ref.game.take_pending_paid_obstacles();
                for obstacle in paid_obstacles {
                    if state_ref.network.is_host {
                        state_ref.network.send_paid_obstacle(obstacle);
                    } else {
                        state_ref.network.send_paid_obstacle_to_supernode(obstacle);
                    }
                }

                let incoming_paid = state_ref.network.take_paid_obstacles();
                for (sender, obstacle) in incoming_paid {
                    let from_supernode = state_ref.network.is_host
                        || sender == "sync"
                        || state_ref.network.supernode_id.is_none()
                        || state_ref.network.is_supernode_sender(&sender);
                    if !from_supernode {
                        continue;
                    }

                    if let Some(peer_id) = state_ref.network.resolve_peer_id(&sender) {
                        state_ref.network.record_paid_obstacle_confirmation(obstacle.proof_hash, peer_id);
                    }

                    let verified = state_ref.network.verify_paid_obstacle(&obstacle);
                    if verified {
                        if let Some(local_id) = state_ref.network.local_peer_id {
                            state_ref.network.record_paid_obstacle_confirmation(obstacle.proof_hash, local_id);
                        }
                        state_ref.network.send_paid_obstacle_ack(net::PaidObstacleAck {
                            proof_hash: obstacle.proof_hash,
                        });

                        if state_ref.network.paid_obstacle_confirmation_count(obstacle.proof_hash) >= 2 {
                            state_ref.game.apply_paid_obstacle(obstacle);
                        } else {
                            state_ref.game.store_paid_obstacle_candidate(obstacle);
                        }
                    } else {
                        state_ref.network.mark_supernode_bad(frame_count);
                        state_ref.game.remove_paid_obstacle(obstacle.proof_hash);
                    }
                }

                let incoming_acks = state_ref.network.take_paid_obstacle_acks();
                for (sender, ack) in incoming_acks {
                    if let Some(peer_id) = state_ref.network.resolve_peer_id(&sender) {
                        state_ref.network.record_paid_obstacle_confirmation(ack.proof_hash, peer_id);
                    }
                }

                let pending_hashes = state_ref.game.pending_paid_obstacle_hashes();
                for hash in pending_hashes {
                    if state_ref.network.paid_obstacle_confirmation_count(hash) >= 2 {
                        if let Some(obstacle) = state_ref.game.take_paid_obstacle_candidate(hash) {
                            state_ref.game.apply_paid_obstacle(obstacle);
                        }
                    }
                }

                if state_ref.network.is_host {
                    let cannon_shots = state_ref.game.take_pending_cannon_shots();
                    for shot in cannon_shots {
                        state_ref.network.send_cannon_shot(shot);
                    }
                } else {
                    let cannon_shots = state_ref.network.take_cannon_shots();
                    for shot in cannon_shots {
                        state_ref.game.projectiles.spawn(
                            crate::math::Vec2::new(shot.x, shot.y),
                            crate::math::Vec2::new(shot.vx, shot.vy),
                            80,
                        );
                    }
                }

                // AUTHORITATIVE HOST MODEL:
                // Host runs the enemy simulation and sends positions to all clients.
                // Clients do NOT run enemy AI - they only receive and interpolate positions.
                // This ensures all players see enemies in the same locations.
                //
                // VISUAL CONSISTENCY: Both host and client snapshot enemy positions at sync rate
                // so they see the same "choppiness" - no unfair advantage for host

                if state_ref.network.is_host {
                    // Host: Send enemy positions every 6 frames (~10 updates/sec)
                    // This is the authoritative state that all clients will use
                    if frame_count % 6 == 0 {
                        let enemy_sync = state_ref.game.create_enemy_sync();
                        state_ref.network.send_enemy_sync(enemy_sync);
                        // Also snapshot for local rendering at same rate
                        state_ref.game.snapshot_enemies_for_render();
                    }
                } else {
                    // Client: Always apply enemy sync from host (this IS the authoritative state)
                    if let Some(sync) = state_ref.network.take_enemy_sync() {
                        state_ref.game.apply_enemy_sync(&sync);
                        state_ref.game.clear_respawn_sync();
                        // Snapshot the received positions for rendering
                        state_ref.game.snapshot_enemies_for_render();
                        state_ref.network.mark_enemy_sync_received(frame_count);
                    }
                }

                if !state_ref.network.is_host && state_ref.network.supernode_is_stale(frame_count) {
                    state_ref.network.mark_supernode_bad(frame_count);
                }
            }

            // Ensure audio context is active after user interaction
            if !state_ref.game.sound_events.is_empty() {
                state_ref.audio.ensure_context();
            }

            // Process sound events
            for event in state_ref.game.sound_events.iter() {
                match event {
                    game::SoundEvent::Attack => state_ref.audio.play_attack(),
                    game::SoundEvent::Block => state_ref.audio.play_block(),
                    game::SoundEvent::Deflect => state_ref.audio.play_deflect(),
                    game::SoundEvent::Phase => state_ref.audio.play_phase(),
                    game::SoundEvent::EnemyKill => state_ref.audio.play_enemy_kill(),
                    game::SoundEvent::Hit => state_ref.audio.play_hit(),
                    game::SoundEvent::Death => state_ref.audio.play_death(),
                    game::SoundEvent::Explosion => state_ref.audio.play_explosion(),
                    game::SoundEvent::MenuSelect => state_ref.audio.play_menu_select(),
                }
            }

            // Send player state every 3 frames (~20 updates/sec at 60fps)
            state_ref.send_counter += 1;
            if state_ref.send_counter >= 3 {
                state_ref.send_counter = 0;
                let player = &state_ref.game.player;
                let player_state = PlayerState::new(
                    player.pos,
                    player.look_dir,
                    player.move_dir,
                    player.alive,
                    player.is_attacking(),
                    player.blocking,
                    player.is_phasing(),
                );
                state_ref.network.send_player_state(player_state);
            }

            if in_multiplayer_room && state_ref.game.scene == Scene::Game {
                state_ref.input_send_counter += 1;
                if state_ref.input_send_counter >= 2 {
                    state_ref.input_send_counter = 0;
                    let frame = state_ref.game.frame_count;
                    let input_frame = net::InputFrame {
                        frame,
                        input: input_snapshot.get_raw(),
                    };
                    state_ref.network.send_input_frame(input_frame);
                }
            }
        }
        {
            let state_ref = state.borrow();
            state_ref.renderer.render(&state_ref.game, &state_ref.network);
        }

        window_clone
            .request_animation_frame(f.borrow().as_ref().unwrap().as_ref().unchecked_ref())
            .expect("failed to request animation frame");
    }));

    window
        .request_animation_frame(g.borrow().as_ref().unwrap().as_ref().unchecked_ref())
        .expect("failed to request animation frame");

    Ok(())
}

// Allow dead code for items that will be used in future phases (GGRS, multiplayer, etc.)
#![allow(dead_code)]

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

mod audio;
mod entities;
mod game;
mod input;
mod math;
mod net;
mod payment;
mod render;
mod world;

use audio::Audio;
use game::{Game, Scene};
use input::Input;
use net::{IceConfig, NetworkSession, NetworkState, PaidNameReservation, PlayerState};
use render::Renderer;

// Default signaling server - using Johan Helsing's public matchbox server
// This is the same server used by Extreme Bevy and other matchbox games
const DEFAULT_SIGNALING_SERVER: &str = "wss://match-0-13.helsing.studio";
const NAME_OWNER_SEED_KEY: &str = "slime_name_owner_seed_v1";
const NAME_RESERVATIONS_KEY: &str = "slime_name_reservations_v1";

// Configurable server URL (can be set via JS)
thread_local! {
    static SIGNALING_SERVER: RefCell<String> = RefCell::new(DEFAULT_SIGNALING_SERVER.to_string());
    static ICE_CONFIG: RefCell<IceConfig> = RefCell::new(IceConfig::default());
}

// Thread-local storage for game state accessible from JS
thread_local! {
    static GAME_STATE: RefCell<Option<Rc<RefCell<GameState>>>> = RefCell::new(None);
    // Separate input buffer to avoid borrow conflicts - event handlers write here,
    // game loop reads and clears each frame
    static INPUT_BUFFER: RefCell<InputBuffer> = RefCell::new(InputBuffer::new());
    /// True while a requestAnimationFrame callback is queued. The hidden-tab
    /// watchdog must not queue extras, or every visibility change would
    /// multiply the tick chains.
    static RAF_PENDING: Cell<bool> = const { Cell::new(false) };
    /// Wall-clock ms of the last game tick (rAF or watchdog driven).
    static LAST_TICK_MS: Cell<f64> = const { Cell::new(0.0) };
    /// The closure registered with requestAnimationFrame (clears RAF_PENDING
    /// then runs the game tick).
    static RAF_SHIM: RefCell<Option<Closure<dyn FnMut()>>> = const { RefCell::new(None) };
    /// Epoch for the wall-clock network frame counter.
    static NET_CLOCK_START_MS: Cell<f64> = const { Cell::new(0.0) };
    // Debug command queue (fed via JS console or tooling)
    static DEBUG_COMMANDS: RefCell<Vec<String>> = RefCell::new(Vec::new());
    static TOUCH_STATE: RefCell<TouchState> = RefCell::new(TouchState::new());
    static MOBILE_INPUT: RefCell<Option<web_sys::HtmlInputElement>> = RefCell::new(None);
}

/// Buffer for input events that event handlers can write to without conflicting
/// with the main game state borrow
struct InputBuffer {
    /// Ordered key events: (code, is_down). Order matters — automation and
    /// fast taps can enqueue an up and a down for the same key within one
    /// frame, and applying downs before ups used to silently eat held keys.
    key_events: Vec<(String, bool)>,
    chars: Vec<char>,
    backspace: bool,
    escape: bool,
    enter: bool,
    tab: bool,
    mute_toggle: bool, // M key to toggle audio mute
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
    let button_radius = 36.0;
    let attack_center = crate::math::Vec2::new((width - 80.0) as f32, (height - 130.0) as f32);
    let phase_center = crate::math::Vec2::new((width - 200.0) as f32, (height - 60.0) as f32);
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
            key_events: Vec::new(),
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
        self.key_events.clear();
        self.chars.clear();
        self.backspace = false;
        self.escape = false;
        self.enter = false;
        self.tab = false;
        self.mute_toggle = false;
        self.click = None;
    }
}

fn queue_key_down(buffer: &mut InputBuffer, code: &str, key: &str) {
    match code {
        "Backspace" => buffer.backspace = true,
        "Escape" => buffer.escape = true,
        "Enter" => buffer.enter = true,
        "Tab" => buffer.tab = true,
        // Arrow keys and modifiers are gameplay/navigation keys only.
        "ArrowUp" | "ArrowDown" | "ArrowLeft" | "ArrowRight" | "ShiftLeft" | "ShiftRight"
        | "ControlLeft" | "ControlRight" | "AltLeft" | "AltRight" | "MetaLeft" | "MetaRight" => {
            buffer.key_events.push((code.to_string(), true));
        }
        _ => {
            buffer.key_events.push((code.to_string(), true));
            if key.len() == 1 {
                if let Some(c) = key.chars().next() {
                    if c.is_ascii() && !c.is_ascii_control() {
                        buffer.chars.push(c);
                    }
                }
            }
        }
    }
}

fn queue_key_up(buffer: &mut InputBuffer, code: &str) {
    if !code.is_empty() {
        buffer.key_events.push((code.to_string(), false));
    }
}

fn automation_key_from_id(key_id: u32) -> Option<(&'static str, &'static str)> {
    match key_id {
        1 => Some(("ArrowUp", "ArrowUp")),
        2 => Some(("ArrowDown", "ArrowDown")),
        3 => Some(("ArrowLeft", "ArrowLeft")),
        4 => Some(("ArrowRight", "ArrowRight")),
        5 => Some(("KeyW", "w")),
        6 => Some(("KeyA", "a")),
        7 => Some(("KeyS", "s")),
        8 => Some(("KeyD", "d")),
        9 => Some(("Space", " ")),
        10 => Some(("Enter", "Enter")),
        11 => Some(("Backspace", "Backspace")),
        12 => Some(("Tab", "Tab")),
        13 => Some(("Escape", "Escape")),
        _ => None,
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

fn load_or_create_name_owner_seed() -> String {
    let seed_from_storage = web_sys::window()
        .and_then(|window| window.local_storage().ok().flatten())
        .and_then(|storage| storage.get_item(NAME_OWNER_SEED_KEY).ok().flatten())
        .filter(|seed| !seed.trim().is_empty());

    if let Some(seed) = seed_from_storage {
        return seed;
    }

    let now = js_sys::Date::now() as u64;
    let rand_a = (js_sys::Math::random() * (u32::MAX as f64)) as u32;
    let rand_b = (js_sys::Math::random() * (u32::MAX as f64)) as u32;
    let seed = format!("slime-owner-{now:016x}-{rand_a:08x}-{rand_b:08x}");

    if let Some(window) = web_sys::window() {
        if let Ok(Some(storage)) = window.local_storage() {
            let _ = storage.set_item(NAME_OWNER_SEED_KEY, &seed);
        }
    }

    seed
}

fn load_or_create_name_owner_hash() -> u64 {
    let seed = load_or_create_name_owner_seed();
    NetworkSession::hash_name_owner_seed(&seed)
}

fn hash_to_hex(hash: &[u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(64);
    for byte in hash {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn parse_hex32(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    let bytes = hex.as_bytes();
    for i in 0..32 {
        let hi = bytes[i * 2];
        let lo = bytes[i * 2 + 1];
        let hi = (hi as char).to_digit(16)? as u8;
        let lo = (lo as char).to_digit(16)? as u8;
        out[i] = (hi << 4) | lo;
    }
    Some(out)
}

fn encode_name_reservations(reservations: &[PaidNameReservation]) -> String {
    let mut out = String::new();
    for reservation in reservations {
        let name = NetworkSession::normalize_player_name(&reservation.name_string());
        if name.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!(
            "{}:{}:{}:{}",
            name,
            reservation.owner_hash,
            reservation.nonce,
            hash_to_hex(&reservation.proof_hash)
        ));
    }
    out
}

fn decode_name_reservations(encoded: &str) -> Vec<PaidNameReservation> {
    let mut out = Vec::new();
    for line in encoded.lines() {
        let mut parts = line.split(':');
        let name = match parts.next() {
            Some(v) => v.trim(),
            None => continue,
        };
        let owner_hash = match parts.next().and_then(|v| v.parse::<u64>().ok()) {
            Some(v) if v != 0 => v,
            _ => continue,
        };
        let nonce = match parts.next().and_then(|v| v.parse::<u32>().ok()) {
            Some(v) => v,
            None => continue,
        };
        let proof_hash = match parts.next().and_then(parse_hex32) {
            Some(v) => v,
            None => continue,
        };
        if parts.next().is_some() {
            continue;
        }
        let normalized = NetworkSession::normalize_player_name(name);
        let reservation =
            PaidNameReservation::from_name(owner_hash, &normalized, nonce, proof_hash);
        out.push(reservation);
    }
    out
}

fn load_saved_name_reservations() -> Vec<PaidNameReservation> {
    let raw = web_sys::window()
        .and_then(|window| window.local_storage().ok().flatten())
        .and_then(|storage| storage.get_item(NAME_RESERVATIONS_KEY).ok().flatten())
        .unwrap_or_default();
    decode_name_reservations(&raw)
}

fn save_name_reservations(reservations: &[PaidNameReservation]) {
    if let Some(window) = web_sys::window() {
        if let Ok(Some(storage)) = window.local_storage() {
            let encoded = encode_name_reservations(reservations);
            let _ = storage.set_item(NAME_RESERVATIONS_KEY, &encoded);
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
    network.set_local_name_owner_hash(load_or_create_name_owner_hash());
    for reservation in load_saved_name_reservations() {
        if network.verify_paid_name_reservation(&reservation) {
            let _ = network.apply_paid_name_reservation(reservation);
        }
    }
    persist_name_reservation_cache(&network);
    if let Some(saved_name) = load_saved_player_name() {
        game.player_name = saved_name.clone();
        network.set_player_name(&saved_name);
    }
    if let Some(updated) = network.ensure_local_name_not_reserved_by_other() {
        game.player_name = updated;
        save_player_name(&game.player_name);
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
        last_enemy_sync_sent_frame: 0,
        last_enemy_intro_sent_frame: 0,
        last_enemy_intro_wave: 0,
        last_tick_net_frame: 0,
        throttled_ticks: 0,
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
            if !validate_title_name_for_room_action(&mut state_ref) {
                return String::new();
            }
            let server = signaling_server_url();
            let ice = ice_config();
            let code = state_ref.network.create_room(&server, &ice);
            // The room code seeds world generation (seed_from_room_code), so
            // the creator must hold it too or they generate a different world
            // than every joiner. The in-game menu path already does this.
            state_ref.game.room_code_input = code.clone();
            code
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
            if !validate_title_name_for_room_action(&mut state_ref) {
                return;
            }
            let server = signaling_server_url();
            let ice = ice_config();
            state_ref.network.join_room(&server, room_code, &ice);
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

/// Set ICE server URLs and optional auth for TURN.
/// `urls_csv` supports comma-separated values, e.g.:
/// "stun:stun.l.google.com:19302,turn:turn.example.com:3478?transport=udp"
/// Pass "none" to use no ICE servers at all (host candidates only — LAN play
/// or automated tests where STUN is unreachable).
#[wasm_bindgen]
pub fn set_ice_servers(urls_csv: &str, username: &str, credential: &str) {
    let disable_ice = urls_csv.trim().eq_ignore_ascii_case("none");
    let urls: Vec<String> = urls_csv
        .split(',')
        .map(|u| u.trim())
        .filter(|u| !u.is_empty())
        .map(ToString::to_string)
        .collect();
    ICE_CONFIG.with(|cfg| {
        let mut cfg = cfg.borrow_mut();
        cfg.urls = if disable_ice {
            Vec::new()
        } else if urls.is_empty() {
            IceConfig::default().urls
        } else {
            urls
        };
        cfg.username = if username.trim().is_empty() {
            None
        } else {
            Some(username.trim().to_string())
        };
        cfg.credential = if credential.trim().is_empty() {
            None
        } else {
            Some(credential.trim().to_string())
        };
    });
}

/// Enable TURN fallback while keeping default STUN servers.
#[wasm_bindgen]
pub fn set_turn_fallback(url: &str, username: &str, credential: &str) {
    let turn_url = url.trim();
    if turn_url.is_empty() {
        return;
    }
    ICE_CONFIG.with(|cfg| {
        let mut cfg = cfg.borrow_mut();
        if cfg.urls.is_empty() {
            cfg.urls = IceConfig::default().urls;
        }
        if !cfg.urls.iter().any(|u| u == turn_url) {
            cfg.urls.push(turn_url.to_string());
        }
        cfg.username = if username.trim().is_empty() {
            None
        } else {
            Some(username.trim().to_string())
        };
        cfg.credential = if credential.trim().is_empty() {
            None
        } else {
            Some(credential.trim().to_string())
        };
    });
}

/// Reset ICE servers back to default STUN-only configuration.
#[wasm_bindgen]
pub fn reset_ice_servers() {
    ICE_CONFIG.with(|cfg| {
        *cfg.borrow_mut() = IceConfig::default();
    });
}

/// Returns the currently configured ICE URLs as comma-separated text.
#[wasm_bindgen]
pub fn get_ice_servers() -> String {
    ICE_CONFIG.with(|cfg| cfg.borrow().urls.join(","))
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

fn scene_name(scene: Scene) -> &'static str {
    match scene {
        Scene::Title => "title",
        Scene::Game => "game",
        Scene::GameOver => "game_over",
    }
}

fn network_state_name(state: &NetworkState) -> String {
    match state {
        NetworkState::Disconnected => "disconnected".to_string(),
        NetworkState::Connecting => "connecting".to_string(),
        NetworkState::WaitingForPeers => "waiting".to_string(),
        NetworkState::Connected => "connected".to_string(),
        NetworkState::Error(e) => format!("error:{e}"),
    }
}

/// Testing automation entry point: enqueue one key-down event.
/// Uses the same internal input path as browser events.
#[wasm_bindgen]
pub fn test_input_key_down(code: &str, key: &str) {
    let key_text = if key.is_empty() { code } else { key };
    INPUT_BUFFER.with(|buf| {
        let mut buf = buf.borrow_mut();
        queue_key_down(&mut buf, code, key_text);
    });
}

/// Testing automation entry point: enqueue one key-up event.
#[wasm_bindgen]
pub fn test_input_key_up(code: &str) {
    INPUT_BUFFER.with(|buf| {
        let mut buf = buf.borrow_mut();
        queue_key_up(&mut buf, code);
    });
}

/// Testing automation entry point: enqueue ASCII text characters.
#[wasm_bindgen]
pub fn test_input_text(text: &str) {
    INPUT_BUFFER.with(|buf| {
        let mut buf = buf.borrow_mut();
        for c in text.chars() {
            if c.is_ascii() && !c.is_ascii_control() {
                buf.chars.push(c);
            }
        }
    });
}

/// Testing automation entry point: enqueue one key-down event using numeric key ids.
/// This avoids JS string ABI edge-cases when automation runs against raw wasm bindings.
#[wasm_bindgen]
pub fn test_input_key_id_down(key_id: u32) {
    if let Some((code, key)) = automation_key_from_id(key_id) {
        INPUT_BUFFER.with(|buf| {
            let mut buf = buf.borrow_mut();
            queue_key_down(&mut buf, code, key);
        });
    }
}

/// Testing automation entry point: enqueue one key-up event using numeric key ids.
#[wasm_bindgen]
pub fn test_input_key_id_up(key_id: u32) {
    if let Some((code, _)) = automation_key_from_id(key_id) {
        INPUT_BUFFER.with(|buf| {
            let mut buf = buf.borrow_mut();
            queue_key_up(&mut buf, code);
        });
    }
}

/// Testing automation entry point: enqueue one printable ASCII character by codepoint.
#[wasm_bindgen]
pub fn test_input_ascii_char(codepoint: u32) {
    if let Some(c) = char::from_u32(codepoint) {
        if c.is_ascii() && !c.is_ascii_control() {
            INPUT_BUFFER.with(|buf| {
                let mut buf = buf.borrow_mut();
                buf.chars.push(c);
            });
        }
    }
}

/// Testing automation entry point: enqueue one click event in canvas coordinates.
#[wasm_bindgen]
pub fn test_input_click(x: f64, y: f64) {
    INPUT_BUFFER.with(|buf| {
        buf.borrow_mut().click = Some((x, y));
    });
}

/// Clears pending buffered input/touch automation state.
#[wasm_bindgen]
pub fn test_input_reset() {
    INPUT_BUFFER.with(|buf| {
        buf.borrow_mut().clear();
    });
    TOUCH_STATE.with(|state| {
        *state.borrow_mut() = TouchState::new();
    });
}

/// Queues a room join from the current title-screen room input.
/// Test-only automation helper to avoid fragile coordinate clicks.
#[wasm_bindgen]
pub fn test_join_current_room() {
    GAME_STATE.with(|gs| {
        if let Some(state) = gs.borrow().as_ref() {
            let mut state_ref = state.borrow_mut();
            if state_ref.game.scene == Scene::Title && state_ref.game.room_code_input.len() >= 4 {
                state_ref.game.queued_join_room = true;
                state_ref.game.text_input_active = false;
            }
        }
    });
}

/// Test-only automation helper to start gameplay without string-based debug commands.
#[wasm_bindgen]
pub fn test_debug_start_game() {
    GAME_STATE.with(|gs| {
        if let Some(state) = gs.borrow().as_ref() {
            state.borrow_mut().game.debug_start_game();
        }
    });
}

/// Test-only automation helper to clear local enemies without string-based debug commands.
#[wasm_bindgen]
pub fn test_debug_clear_enemies() {
    GAME_STATE.with(|gs| {
        if let Some(state) = gs.borrow().as_ref() {
            let mut state_ref = state.borrow_mut();
            let _ = state_ref.game.apply_debug_command("clear_enemies");
        }
    });
}

/// Test-only automation helper to teleport the local player without string-based debug commands.
#[wasm_bindgen]
pub fn test_debug_teleport(x: f32, y: f32) {
    GAME_STATE.with(|gs| {
        if let Some(state) = gs.borrow().as_ref() {
            let mut state_ref = state.borrow_mut();
            let _ = state_ref
                .game
                .apply_debug_command(&format!("teleport {x} {y}"));
        }
    });
}

/// Compact runtime snapshot for automation assertions.
/// Format: `scene=...;network=...;room=...;players=...;map_open=...;player_list_open=...;chat_open=...`
#[wasm_bindgen]
pub fn test_runtime_state() -> String {
    GAME_STATE.with(|gs| {
        let Ok(game_state) = gs.try_borrow() else {
            return "scene=busy;network=busy;room=;players=0;map_open=false;player_list_open=false;chat_open=false".to_string();
        };
        if let Some(state) = game_state.as_ref() {
            let Ok(state_ref) = state.try_borrow() else {
                return "scene=busy;network=busy;room=;players=0;map_open=false;player_list_open=false;chat_open=false".to_string();
            };
            let scene = scene_name(state_ref.game.scene);
            let net = network_state_name(&state_ref.network.state);
            let room = state_ref.network.room_code.as_str();
            let players = state_ref.network.peer_count() + 1;
            let map_open = state_ref.game.map_open;
            let player_list_open = state_ref.game.player_list_open;
            let chat_open = state_ref.game.chat_open;
            format!(
                "scene={scene};network={net};room={room};players={players};map_open={map_open};player_list_open={player_list_open};chat_open={chat_open}"
            )
        } else {
            "scene=uninitialized;network=uninitialized;room=;players=0;map_open=false;player_list_open=false;chat_open=false".to_string()
        }
    })
}

/// Test/debug: full dump of alive enemies + area-authority diagnostics as
/// JSON, for cross-window jitter measurement in browser-driven tests.
/// Entries are [type, id, x, y]; types: 0 spider, 1 cannon, 2 snake,
/// 3 wisp, 4 guardian.
#[wasm_bindgen]
pub fn test_enemy_positions() -> String {
    GAME_STATE.with(|gs| {
        let Ok(game_state) = gs.try_borrow() else {
            return "{\"busy\":true}".to_string();
        };
        if let Some(state) = game_state.as_ref() {
            let Ok(state_ref) = state.try_borrow() else {
                return "{\"busy\":true}".to_string();
            };
            let game = &state_ref.game;
            let mut parts: Vec<String> = Vec::new();
            for e in game.spiders.iter().filter(|e| e.alive) {
                parts.push(format!("[0,{},{:.1},{:.1}]", e.id, e.pos.x, e.pos.y));
            }
            for e in game.cannons.iter().filter(|e| e.alive) {
                parts.push(format!("[1,{},{:.1},{:.1}]", e.id, e.pos.x, e.pos.y));
            }
            for e in game.snakes.iter().filter(|e| e.alive) {
                parts.push(format!("[2,{},{:.1},{:.1}]", e.id, e.pos.x, e.pos.y));
            }
            for e in game.wisps.iter().filter(|e| e.alive) {
                parts.push(format!("[3,{},{:.1},{:.1}]", e.id, e.pos.x, e.pos.y));
            }
            for e in game.guardians.iter().filter(|e| e.alive) {
                parts.push(format!("[4,{},{:.1},{:.1}]", e.id, e.pos.x, e.pos.y));
            }
            format!(
                "{{\"tick\":{},\"wave\":{},\"px\":{:.1},\"py\":{:.1},\"areas\":{},\"enemies\":[{}]}}",
                game.frame_count,
                game.wave,
                game.player.pos.x,
                game.player.pos.y,
                state_ref.network.area_authority_debug(),
                parts.join(",")
            )
        } else {
            "{\"uninitialized\":true}".to_string()
        }
    })
}

/// Test/debug: enemy counts and a few live positions, to observe whether the
/// authoritative simulation and enemy sync are actually moving things.
#[wasm_bindgen]
pub fn test_enemy_snapshot() -> String {
    GAME_STATE.with(|gs| {
        let Ok(game_state) = gs.try_borrow() else {
            return "busy".to_string();
        };
        if let Some(state) = game_state.as_ref() {
            let Ok(state_ref) = state.try_borrow() else {
                return "busy".to_string();
            };
            let game = &state_ref.game;
            let alive = |count: usize| -> String { count.to_string() };
            let mut out = format!(
                "tick={};sync_tick={};wave={};spiders={};cannons={};snakes={};wisps={}",
                game.frame_count,
                game.last_enemy_sync_tick,
                game.wave,
                alive(game.spiders.iter().filter(|e| e.alive).count()),
                alive(game.cannons.iter().filter(|e| e.alive).count()),
                alive(game.snakes.iter().filter(|e| e.alive).count()),
                alive(game.wisps.iter().filter(|e| e.alive).count()),
            );
            for spider in game.spiders.iter().filter(|e| e.alive).take(3) {
                out.push_str(&format!(
                    ";spider{}={:.1},{:.1}",
                    spider.id, spider.pos.x, spider.pos.y
                ));
            }
            for snake in game.snakes.iter().filter(|e| e.alive).take(2) {
                out.push_str(&format!(
                    ";snake{}={:.1},{:.1}",
                    snake.id, snake.pos.x, snake.pos.y
                ));
            }
            for wisp in game.wisps.iter().filter(|e| e.alive).take(2) {
                out.push_str(&format!(
                    ";wisp{}={:.1},{:.1}",
                    wisp.id, wisp.pos.x, wisp.pos.y
                ));
            }
            out
        } else {
            "uninitialized".to_string()
        }
    })
}

/// Test/debug: force a relay-tree fanout (0 clears). Lets small test rooms
/// form deep chains to exercise depth >= 2 relay paths.
#[wasm_bindgen]
pub fn test_set_fanout(fanout: u32) {
    GAME_STATE.with(|gs| {
        let Ok(game_state) = gs.try_borrow() else {
            return;
        };
        if let Some(state) = game_state.as_ref() {
            let Ok(mut state_ref) = state.try_borrow_mut() else {
                return;
            };
            state_ref.network.set_fanout_override(fanout as usize);
        }
    })
}

/// Test/debug: root-failover predicate internals.
#[wasm_bindgen]
pub fn test_failover_diag() -> String {
    GAME_STATE.with(|gs| {
        let Ok(game_state) = gs.try_borrow() else {
            return "busy".to_string();
        };
        if let Some(state) = game_state.as_ref() {
            let Ok(state_ref) = state.try_borrow() else {
                return "busy".to_string();
            };
            state_ref.network.failover_debug()
        } else {
            "uninitialized".to_string()
        }
    })
}

/// Focused network diagnostics for multi-device validation.
/// Format:
/// `network=...;room=...;ice=...;remote_players=...;known_peers=...;desired_peers=...;desired_ids=...;discovery_attached=...;relay_epoch=...;is_host=...;local_peer=...;super_root=...;supernode=...;local_name=...;rx=...;dropped=...;remote_ids=...;remote_names=...`
#[wasm_bindgen]
pub fn test_network_diag() -> String {
    GAME_STATE.with(|gs| {
        let Ok(game_state) = gs.try_borrow() else {
            return "network=busy;room=;ice=;remote_players=0;known_peers=0;desired_peers=0;desired_ids=none;discovery_attached=false;relay_epoch=0;is_host=false;local_peer=none;super_root=none;supernode=none;local_name=none;rx=0;dropped=0;remote_ids=none;remote_names=none".to_string();
        };
        if let Some(state) = game_state.as_ref() {
            let Ok(state_ref) = state.try_borrow() else {
                return "network=busy;room=;ice=;remote_players=0;known_peers=0;desired_peers=0;desired_ids=none;discovery_attached=false;relay_epoch=0;is_host=false;local_peer=none;super_root=none;supernode=none;local_name=none;rx=0;dropped=0;remote_ids=none;remote_names=none".to_string();
            };
            let net = network_state_name(&state_ref.network.state);
            let room = state_ref.network.room_code.as_str();
            let ice = get_ice_servers();
            let remote_players = state_ref.network.peer_count();
            let known_peers = state_ref.network.known_peer_count();
            let desired_peers = state_ref.network.desired_peer_count();
            let desired_ids = state_ref.network.desired_peer_debug();
            let discovery_attached = state_ref.network.discovery_attached();
            let relay_epoch = state_ref.network.relay_epoch();
            let is_host = state_ref.network.is_host;
            let local_peer = state_ref
                .network
                .local_peer_id
                .map(|id| format!("{:?}", id))
                .unwrap_or_else(|| "none".to_string());
            let supernode = state_ref
                .network
                .supernode_id
                .map(|id| format!("{:?}", id))
                .unwrap_or_else(|| "none".to_string());
            let super_root = state_ref
                .network
                .super_root_id
                .map(|id| format!("{:?}", id))
                .unwrap_or_else(|| "none".to_string());
            let local_name = state_ref.network.local_display_name();
            let rx = state_ref.network.relay_telemetry.recv_messages;
            let dropped = state_ref.network.relay_telemetry.dropped_messages;
            let mut remote_ids: Vec<String> = state_ref
                .network
                .remote_players
                .keys()
                .cloned()
                .collect();
            remote_ids.sort();
            let remote_ids = if remote_ids.is_empty() {
                "none".to_string()
            } else {
                remote_ids.join(",")
            };
            let mut remote_names: Vec<String> = state_ref
                .network
                .remote_players
                .keys()
                .map(|peer_id| {
                    let display = state_ref.network.display_name_for_peer_id(peer_id);
                    format!("{peer_id}:{display}")
                })
                .collect();
            remote_names.sort();
            let remote_names = if remote_names.is_empty() {
                "none".to_string()
            } else {
                remote_names.join(",")
            };
            format!(
                "network={net};room={room};ice={ice};remote_players={remote_players};known_peers={known_peers};desired_peers={desired_peers};desired_ids={desired_ids};discovery_attached={discovery_attached};relay_epoch={relay_epoch};is_host={is_host};local_peer={local_peer};super_root={super_root};supernode={supernode};local_name={local_name};rx={rx};dropped={dropped};remote_ids={remote_ids};remote_names={remote_names}"
            )
        } else {
            "network=uninitialized;room=;ice=;remote_players=0;known_peers=0;desired_peers=0;desired_ids=none;discovery_attached=false;relay_epoch=0;is_host=false;local_peer=none;super_root=none;supernode=none;local_name=none;rx=0;dropped=0;remote_ids=none;remote_names=none".to_string()
        }
    })
}

/// Compact player-position snapshot for sync assertions.
/// Format:
/// `scene=...;local=NAME@x,y;remote=NAME@x,y|NAME@x,y`
#[wasm_bindgen]
pub fn test_player_snapshot() -> String {
    GAME_STATE.with(|gs| {
        let Ok(game_state) = gs.try_borrow() else {
            return "scene=busy;local=none@0.0,0.0;remote=none".to_string();
        };
        if let Some(state) = game_state.as_ref() {
            let Ok(state_ref) = state.try_borrow() else {
                return "scene=busy;local=none@0.0,0.0;remote=none".to_string();
            };
            let scene = scene_name(state_ref.game.scene);
            let local_name = state_ref.network.local_display_name();
            let local_pos = state_ref.game.player.pos;
            let mut remotes: Vec<String> = state_ref
                .network
                .remote_players
                .keys()
                .map(|peer_id| {
                    let display = state_ref.network.display_name_for_peer_id(peer_id);
                    let remote = &state_ref.network.remote_players[peer_id];
                    format!("{display}@{:.1},{:.1}", remote.pos.x, remote.pos.y)
                })
                .collect();
            remotes.sort();
            let remote = if remotes.is_empty() {
                "none".to_string()
            } else {
                remotes.join("|")
            };
            format!(
                "scene={scene};local={local_name}@{:.1},{:.1};remote={remote}",
                local_pos.x, local_pos.y
            )
        } else {
            "scene=uninitialized;local=none@0.0,0.0;remote=none".to_string()
        }
    })
}

fn make_event_id(seed: u64, a: u64, b: u64, c: u64) -> u64 {
    let mut x = seed ^ 0x9e3779b97f4a7c15;
    x = x.wrapping_add(a.wrapping_mul(0xbf58476d1ce4e5b9));
    x = x.rotate_left(27) ^ b.wrapping_mul(0x94d049bb133111eb);
    x = x.wrapping_add(c.wrapping_mul(0x9e3779b97f4a7c15));
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51afd7ed558ccd);
    x ^= x >> 33;
    x
}

/// Helper function to get the signaling server URL
fn signaling_server_url() -> String {
    SIGNALING_SERVER.with(|s| s.borrow().clone())
}

fn ice_config() -> IceConfig {
    ICE_CONFIG.with(|cfg| cfg.borrow().clone())
}

struct GameState {
    game: Game,
    input: Input,
    renderer: Renderer,
    network: NetworkSession,
    audio: Audio,
    send_counter: u32,       // Send network updates every N frames
    input_send_counter: u32, // Send input frames every N frames
    last_supernode_id: Option<matchbox_socket::PeerId>,
    /// Wall-clock net frame when the host last sent an enemy sync. Cadence
    /// must be delta-based: a throttled tab's net clock jumps in fixed steps
    /// (e.g. exactly 30 or 60 per watchdog tick), so `frame % stride == 0`
    /// can lock onto a non-zero residue and never fire again.
    last_enemy_sync_sent_frame: u32,
    /// Low-cadence full host snapshots used only to introduce newly spawned
    /// enemies in delegated areas; receivers ignore known delegated enemies.
    last_enemy_intro_sent_frame: u32,
    last_enemy_intro_wave: u32,
    /// Wall-clock net frame of the previous tick (sim catch-up bookkeeping).
    last_tick_net_frame: u32,
    /// Consecutive heavily-throttled ticks (background tab detection).
    throttled_ticks: u32,
}

fn persist_name_reservation_cache(network: &NetworkSession) {
    let snapshot = network.paid_name_reservations_snapshot();
    save_name_reservations(&snapshot);
}

fn title_name_reservation_block_reason(
    state_ref: &GameState,
    normalized: &str,
    owner_hash: u64,
) -> String {
    let owner_name = state_ref.network.display_name_for_hash(owner_hash);
    if owner_name.eq_ignore_ascii_case("player") {
        format!(
            "{} is reserved by another owner ({:016X}). Choose a different name.",
            normalized, owner_hash
        )
    } else {
        format!(
            "{} is reserved by {} ({:016X}). Choose a different name.",
            normalized, owner_name, owner_hash
        )
    }
}

fn validate_title_name_for_room_action(state_ref: &mut GameState) -> bool {
    let normalized = NetworkSession::normalize_player_name(&state_ref.game.player_name);
    if state_ref.game.player_name != normalized {
        state_ref.game.player_name = normalized.clone();
        save_player_name(&state_ref.game.player_name);
    }
    state_ref.network.set_player_name(&normalized);
    let local_owner_hash = state_ref.network.local_name_owner_hash();
    if local_owner_hash == 0 {
        state_ref.game.title_warning =
            "Name identity unavailable. Reload and try again.".to_string();
        return false;
    }
    if let Some(owner_hash) = state_ref.network.reserved_name_owner_hash(&normalized) {
        if owner_hash != local_owner_hash {
            state_ref.game.title_warning =
                title_name_reservation_block_reason(state_ref, &normalized, owner_hash);
            return false;
        }
    }
    state_ref.game.title_warning.clear();
    true
}

fn setup_input(
    window: &web_sys::Window,
    state: Rc<RefCell<GameState>>,
    canvas: &web_sys::HtmlCanvasElement,
) -> Result<(), JsValue> {
    // Keydown handler - writes to INPUT_BUFFER instead of directly to state
    let keydown = Closure::<dyn FnMut(_)>::new(move |event: web_sys::KeyboardEvent| {
        let code = event.code();
        let key = event.key();

        event.prevent_default();

        INPUT_BUFFER.with(|buf| {
            let mut buf = buf.borrow_mut();
            queue_key_down(&mut buf, &code, &key);
        });
    });
    window.add_event_listener_with_callback("keydown", keydown.as_ref().unchecked_ref())?;
    keydown.forget();

    // Keyup handler - writes to INPUT_BUFFER
    let keyup = Closure::<dyn FnMut(_)>::new(move |event: web_sys::KeyboardEvent| {
        event.prevent_default();
        INPUT_BUFFER.with(|buf| {
            let mut buf = buf.borrow_mut();
            queue_key_up(&mut buf, &event.code());
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
                let input_listener =
                    Closure::<dyn FnMut(_)>::new(move |event: web_sys::InputEvent| {
                        event.prevent_default();
                        let target = event.target().unwrap();
                        let input_el: web_sys::HtmlInputElement = target.dyn_into().unwrap();
                        let value = input_el.value();
                        if value.is_empty() {
                            let input_type = event.input_type();
                            if input_type == "deleteContentBackward"
                                || input_type == "deleteContentForward"
                            {
                                INPUT_BUFFER.with(|buf| {
                                    buf.borrow_mut().backspace = true;
                                });
                            }
                            return;
                        }
                        INPUT_BUFFER.with(|buf| {
                            let mut buf = buf.borrow_mut();
                            for c in value.chars() {
                                if c.is_ascii() && !c.is_ascii_control() {
                                    buf.chars.push(c);
                                }
                            }
                        });
                        input_el.set_value("");
                    });
                input_el.add_event_listener_with_callback(
                    "input",
                    input_listener.as_ref().unchecked_ref(),
                )?;
                input_listener.forget();

                let key_listener =
                    Closure::<dyn FnMut(_)>::new(move |event: web_sys::KeyboardEvent| {
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
                input_el.add_event_listener_with_callback(
                    "keydown",
                    key_listener.as_ref().unchecked_ref(),
                )?;
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
                let mut allow_controls = true;
                let mut map_margin = 0.0;

                let mut map_rect: Option<(f64, f64, f64)> = None;
                {
                    let mut state_ref = touch_state.borrow_mut();
                    if state_ref.game.mobile_mode {
                        if state_ref.game.scene == Scene::GameOver && !state_ref.game.map_open {
                            state_ref.game.map_open = true;
                            handled_ui = true;
                        }
                        if state_ref.game.scene == Scene::Title {
                            let center_x = state_ref.game.width as f64 / 2.0;
                            let name_box_x = center_x - 90.0;
                            let name_box_y = 220.0 - 14.0;
                            let code_box_x = center_x - 90.0;
                            let code_box_y = 260.0 - 14.0;
                            let join_x = center_x + 20.0;
                            let join_y = 260.0 - 14.0;

                            if x >= name_box_x
                                && x <= name_box_x + 180.0
                                && y >= name_box_y
                                && y <= name_box_y + 22.0
                            {
                                state_ref.game.activate_text_input(0);
                                handled_ui = true;
                                focus_input = true;
                            } else if x >= code_box_x
                                && x <= code_box_x + 100.0
                                && y >= code_box_y
                                && y <= code_box_y + 22.0
                            {
                                state_ref.game.activate_text_input(1);
                                handled_ui = true;
                                focus_input = true;
                            } else if x >= join_x
                                && x <= join_x + 70.0
                                && y >= join_y
                                && y <= join_y + 22.0
                            {
                                state_ref.game.queued_join_room = true;
                                state_ref.game.text_input_active = false;
                                handled_ui = true;
                            }
                        } else if (state_ref.game.scene == Scene::Game
                            || state_ref.game.scene == Scene::GameOver)
                            && state_ref.game.map_open
                        {
                            let in_circle = |center: crate::math::Vec2, radius: f64| {
                                let dx = pos.x as f64 - center.x as f64;
                                let dy = pos.y as f64 - center.y as f64;
                                dx * dx + dy * dy <= radius * radius
                            };
                            let (map_left, map_top, map_size) = state_ref.game.map_overlay_rect();
                            map_margin = 60.0;
                            allow_controls = false;
                            map_rect = Some((map_left, map_top, map_size));
                            let layout = touch_layout(canvas_width, canvas_height);
                            if in_circle(layout.zoom_in_center, layout.top_button_radius) {
                                state_ref.game.zoom_map_in();
                                handled_ui = true;
                            } else if in_circle(layout.zoom_out_center, layout.top_button_radius) {
                                state_ref.game.zoom_map_out();
                                handled_ui = true;
                            } else if x < map_left - map_margin
                                || x > map_left + map_size + map_margin
                                || y < map_top - map_margin
                                || y > map_top + map_size + map_margin
                            {
                                state_ref.game.map_open = false;
                                handled_ui = true;
                            } else {
                                let input_x = map_left + 10.0;
                                let input_y = map_top + map_size - 6.0;
                                let button_w = 100.0;
                                let button_h = 20.0;
                                let button_x = map_left + map_size - button_w - 10.0;
                                let button_y = input_y - 28.0;
                                if x >= button_x
                                    && x <= button_x + button_w
                                    && y >= button_y
                                    && y <= button_y + button_h
                                {
                                    state_ref.game.confirm_map_teleport();
                                    handled_ui = true;
                                } else if x >= input_x
                                    && x <= input_x + 140.0
                                    && y >= input_y - 14.0
                                    && y <= input_y + 8.0
                                {
                                    state_ref.game.activate_map_input(0);
                                    handled_ui = true;
                                    focus_input = true;
                                } else if x >= input_x + 170.0
                                    && x <= input_x + 310.0
                                    && y >= input_y - 14.0
                                    && y <= input_y + 8.0
                                {
                                    state_ref.game.activate_map_input(1);
                                    handled_ui = true;
                                    focus_input = true;
                                }
                            }
                        } else if state_ref.game.mobile_mode
                            && (state_ref.game.scene == Scene::Game
                                || state_ref.game.scene == Scene::GameOver)
                        {
                            let in_circle = |center: crate::math::Vec2, radius: f64| {
                                let dx = pos.x as f64 - center.x as f64;
                                let dy = pos.y as f64 - center.y as f64;
                                dx * dx + dy * dy <= radius * radius
                            };
                            if state_ref.game.player_list_open {
                                allow_controls = false;
                            }
                            if in_circle(layout.chat_center, layout.top_button_radius) {
                                state_ref.game.toggle_chat();
                                handled_ui = true;
                                focus_input = state_ref.game.chat_open;
                            } else if !state_ref.game.map_open && !state_ref.game.player_list_open {
                                let map_size = 120.0;
                                let map_padding = 10.0;
                                let map_left =
                                    (state_ref.game.width as f64) - map_size - map_padding;
                                let portrait =
                                    state_ref.game.viewport_height > state_ref.game.viewport_width;
                                let map_top = if state_ref.game.mobile_mode || portrait {
                                    130.0
                                } else {
                                    (state_ref.game.height as f64) - map_size - map_padding
                                };
                                if x >= map_left
                                    && x <= map_left + map_size
                                    && y >= map_top
                                    && y <= map_top + map_size
                                {
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
                                if x >= close_x
                                    && x <= close_x + 18.0
                                    && y >= close_y
                                    && y <= close_y + 18.0
                                {
                                    state_ref.game.toggle_player_list();
                                    handled_ui = true;
                                } else if x >= left
                                    && x <= left + overlay_w
                                    && y >= top
                                    && y <= top + 360.0
                                {
                                    handled_ui = true;
                                    start_list_drag = true;
                                } else {
                                    state_ref.game.toggle_player_list();
                                    handled_ui = true;
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

                    if allow_controls {
                        if state.joystick_id.is_none()
                            && in_circle(layout.stick_center, layout.stick_radius * 1.3)
                        {
                            state.joystick_id = Some(id);
                            state.joystick_center = pos;
                            state.joystick_axis = crate::math::Vec2::ZERO;
                            return;
                        }
                        if state.attack_id.is_none()
                            && in_circle(layout.attack_center, layout.button_radius)
                        {
                            state.attack_id = Some(id);
                            return;
                        }
                        if state.phase_id.is_none()
                            && in_circle(layout.phase_center, layout.button_radius)
                        {
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
                    }

                    if let Some((map_left, map_top, map_size)) = map_rect {
                        let within = x >= map_left
                            && x <= map_left + map_size
                            && y >= map_top
                            && y <= map_top + map_size;
                        let within_margin = x >= map_left - map_margin
                            && x <= map_left + map_size + map_margin
                            && y >= map_top - map_margin
                            && y <= map_top + map_size + map_margin;
                        if within || within_margin {
                            state.map_drag_id = Some(id);
                            state.map_drag_last = pos;
                            state.map_drag_distance = 0.0;
                            state.map_tap_candidate = true;
                            let clamped_x = if x < map_left {
                                map_left
                            } else if x > map_left + map_size {
                                map_left + map_size
                            } else {
                                x
                            };
                            let clamped_y = if y < map_top {
                                map_top
                            } else if y > map_top + map_size {
                                map_top + map_size
                            } else {
                                y
                            };
                            state.map_tap_pos =
                                crate::math::Vec2::new(clamped_x as f32, clamped_y as f32);
                        }
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
                        state.joystick_axis = if max > 0.0 {
                            axis / max
                        } else {
                            crate::math::Vec2::ZERO
                        };
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

/// Network timing is frame-denominated assuming 60/s, but the tick rate is
/// variable (60/s on rAF, ~1/s under the hidden-tab watchdog). Deriving the
/// network's frame clock from wall time keeps every age/timeout/cadence
/// correct regardless of how the loop is being driven.
fn network_clock_frame() -> u32 {
    NET_CLOCK_START_MS.with(|cell| {
        let now = js_sys::Date::now();
        if cell.get() == 0.0 {
            cell.set(now);
        }
        ((now - cell.get()) * 0.06) as u32
    })
}

fn start_game_loop(window: web_sys::Window, state: Rc<RefCell<GameState>>) -> Result<(), JsValue> {
    let f: Rc<RefCell<Option<Closure<dyn FnMut()>>>> = Rc::new(RefCell::new(None));
    let g = f.clone();

    let window_clone = window.clone();
    *g.borrow_mut() = Some(Closure::new(move || {
        LAST_TICK_MS.with(|cell| cell.set(js_sys::Date::now()));
        {
            let mut state_ref = state.borrow_mut();
            let is_mobile = window_clone
                .inner_width()
                .ok()
                .and_then(|w| w.as_f64())
                .unwrap_or(0.0)
                <= 900.0;
            state_ref.game.set_mobile_mode(is_mobile);
            let viewport_w = window_clone
                .inner_width()
                .ok()
                .and_then(|w| w.as_f64())
                .unwrap_or(state_ref.game.width as f64);
            let viewport_h = window_clone
                .inner_height()
                .ok()
                .and_then(|h| h.as_f64())
                .unwrap_or(state_ref.game.height as f64);
            state_ref.game.set_viewport_size(viewport_w, viewport_h);

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
                    buf.key_events.clear();
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

                    buf.key_events.clear();
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
                                if trimmed.to_ascii_lowercase().starts_with("/bind ") {
                                    let cmd = trimmed.trim_start_matches('/').to_string();
                                    match state_ref.game.apply_debug_command(&cmd) {
                                        Ok(message) => state_ref.game.push_chat_line("System".to_string(), message),
                                        Err(err) => state_ref.game.push_chat_line("System".to_string(), err),
                                    }
                                } else if trimmed.to_ascii_lowercase().starts_with("/mute") {
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
                                        let matches = state_ref.network.matching_display_names(target);
                                        if !matches.is_empty() {
                                            state_ref.game.push_chat_line(
                                                "System".to_string(),
                                                format!(
                                                    "Name is ambiguous. Use exact name: {}",
                                                    matches.join(", ")
                                                ),
                                            );
                                        } else {
                                            state_ref.game.push_chat_line(
                                                "System".to_string(),
                                                format!("No player named '{}'.", target),
                                            );
                                        }
                                    }
                                } else if trimmed.to_ascii_lowercase().starts_with("/buyname") {
                                    let requested = trimmed.splitn(2, ' ').nth(1).unwrap_or("").trim();
                                    let owner_hash = state_ref.network.local_name_owner_hash();
                                    if state_ref.network.room_code.is_empty() {
                                        state_ref.game.push_chat_line(
                                            "System".to_string(),
                                            "Join a room first before reserving a name.".to_string(),
                                        );
                                    } else if owner_hash == 0 {
                                        state_ref.game.push_chat_line(
                                            "System".to_string(),
                                            "Chat is still initializing.".to_string(),
                                        );
                                    } else if !payment::support_valid() {
                                        payment::prompt_support();
                                        state_ref.game.push_chat_line(
                                            "System".to_string(),
                                            "Payment required for name reservation. Press 4 to open payments.".to_string(),
                                        );
                                    } else {
                                        let base_name = if requested.is_empty() {
                                            state_ref.game.player_name.clone()
                                        } else {
                                            requested.to_string()
                                        };
                                        let normalized = net::NetworkSession::normalize_player_name(&base_name);
                                        if state_ref.network.is_name_reserved_by_self(&normalized, owner_hash) {
                                            state_ref.game.push_chat_line(
                                                "System".to_string(),
                                                format!("{} is already reserved by you.", normalized),
                                            );
                                        } else if let Some(owner_hash) = state_ref.network.reserved_name_owner_hash(&normalized) {
                                            let owner_name = state_ref.network.display_name_for_hash(owner_hash);
                                            state_ref.game.push_chat_line(
                                                "System".to_string(),
                                                format!("{} is already reserved by {}.", normalized, owner_name),
                                            );
                                        } else {
                                            let reservation = state_ref.network.build_paid_name_reservation(
                                                owner_hash,
                                                &normalized,
                                                state_ref.game.frame_count,
                                            );
                                            state_ref.network.store_paid_name_candidate(reservation);
                                            if state_ref.network.is_host {
                                                state_ref.network.send_paid_name_reservation(reservation);
                                            } else {
                                                state_ref.network.send_paid_name_reservation_to_supernode(reservation);
                                            }
                                            state_ref.game.push_chat_line(
                                                "System".to_string(),
                                                format!("Reservation request sent for {}.", normalized),
                                            );
                                        }
                                    }
                                } else if local_hash == 0 && !state_ref.network.room_code.is_empty() {
                                    state_ref.game.push_chat_line(
                                        "System".to_string(),
                                        "Chat is still initializing.".to_string(),
                                    );
                                } else {
                                    let name = state_ref.network.local_display_name();
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

                    buf.key_events.clear();
                } else {
                    if state_ref.game.player_list_open
                        && (state_ref.game.scene == Scene::Game || state_ref.game.scene == Scene::GameOver)
                    {
                        let blocked = ["KeyZ", "KeyX", "ArrowLeft", "ArrowRight"];
                        buf.key_events
                            .retain(|(code, _)| !blocked.contains(&code.as_str()));
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
                        buf.key_events
                            .retain(|(code, _)| allowed.contains(&code.as_str()));
                    }
                    if (state_ref.game.scene == Scene::Game || state_ref.game.scene == Scene::GameOver)
                        && !state_ref.game.map_open
                    {
                        let mut handled_keys: Vec<String> = Vec::new();
                        for (code, is_down) in &buf.key_events {
                            if !is_down {
                                continue;
                            }
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
                                "KeyB" => {
                                    if state_ref.game.scene == Scene::Game {
                                        state_ref.game.toggle_ability_bind_menu();
                                        handled_keys.push(code.clone());
                                    }
                                }
                                "F3" => {
                                    if state_ref.game.scene == Scene::Game {
                                        state_ref.game.toggle_net_debug_overlay();
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
                            buf.key_events
                                .retain(|(code, is_down)| !*is_down || !handled_keys.contains(code));
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
                        buf.key_events.clear();
                    }

                    if (state_ref.game.scene == Scene::Game || state_ref.game.scene == Scene::GameOver)
                        && state_ref.game.map_open
                        && buf.chars.iter().any(|c| c.is_ascii_digit() || *c == '-' || *c == '.')
                    {
                        state_ref.game.activate_map_input(0);
                        for c in buf.chars.drain(..) {
                            state_ref.game.handle_map_char_input(c);
                        }
                        buf.key_events.clear();
                        buf.clear();
                        return;
                    }
                    // Process key events in arrival order so a same-frame
                    // release+press of the same key nets out to "held".
                    for (code, is_down) in buf.key_events.drain(..) {
                        if !is_down {
                            state_ref.input.key_up(&code);
                            continue;
                        }
                        if state_ref.game.ability_bind_open {
                            if state_ref.game.ability_bind_waiting() {
                                if state_ref.game.handle_ability_bind_key(&code) {
                                    continue;
                                }
                            } else if code == "ArrowLeft" {
                                state_ref.game.cycle_ability_bind_selection(-1);
                                continue;
                            } else if code == "ArrowRight" {
                                state_ref.game.cycle_ability_bind_selection(1);
                                continue;
                            } else if code == "Enter" {
                                state_ref.game.start_ability_rebind();
                                continue;
                            } else if code == "Escape" {
                                state_ref.game.toggle_ability_bind_menu();
                                continue;
                            }
                        }
                        if code == "Digit4" {
                            crate::payment::open_payment_modal();
                            continue;
                        }
                        if state_ref.game.handle_ability_key(&code) {
                            continue;
                        }
                        state_ref.input.key_down(&code);
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
                            let (map_left, map_top, map_size) = state_ref.game.map_overlay_rect();
                            let map_margin = 60.0;
                            let close_x = map_left + map_size - 26.0;
                            let close_y = map_top + 8.0;
                            if x >= close_x && x <= close_x + 18.0 && y >= close_y && y <= close_y + 18.0 {
                                state_ref.game.map_open = false;
                                return;
                            }
                            if x < map_left - map_margin
                                || x > map_left + map_size + map_margin
                                || y < map_top - map_margin
                                || y > map_top + map_size + map_margin
                            {
                                state_ref.game.map_open = false;
                                return;
                            }
                            let clamped_x = if x < map_left {
                                map_left
                            } else if x > map_left + map_size {
                                map_left + map_size
                            } else {
                                x
                            };
                            let clamped_y = if y < map_top {
                                map_top
                            } else if y > map_top + map_size {
                                map_top + map_size
                            } else {
                                y
                            };
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
                                state_ref.game.handle_map_click(clamped_x, clamped_y);
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
                            let portrait = state_ref.game.viewport_height > state_ref.game.viewport_width;
                            let map_top = if state_ref.game.mobile_mode || portrait { 130.0 } else { (state_ref.game.height as f64) - map_size - map_padding };
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
                    if state_ref.game.scene == Scene::Game && !state_ref.game.map_open {
                        if state_ref.game.handle_ability_bar_click(x, y) {
                            buf.click = None;
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
                    let mut down_mask = 0u16;
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
                        if state_ref.game.scene == Scene::Game
                            || state_ref.game.scene == Scene::GameOver
                        {
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
                            let pan_dx = if state_ref.game.mobile_mode {
                                -state.map_drag_delta.x
                            } else {
                                state.map_drag_delta.x
                            };
                            let pan_dy = if state_ref.game.mobile_mode {
                                -state.map_drag_delta.y
                            } else {
                                state.map_drag_delta.y
                            };
                            state_ref.game.pan_map_by_screen_delta(pan_dx, pan_dy);
                            state.map_drag_delta = crate::math::Vec2::ZERO;
                        }
                        if state.map_tap_candidate && state.map_drag_distance < 8.0 {
                            state_ref.game.handle_map_click(
                                state.map_tap_pos.x as f64,
                                state.map_tap_pos.y as f64,
                            );
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
                    let wants_text = state_ref.game.scene == Scene::Title
                        && state_ref.game.text_input_active
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
            let commands: Vec<String> =
                DEBUG_COMMANDS.with(|queue| queue.borrow_mut().drain(..).collect());
            for command in commands {
                match state_ref.game.apply_debug_command(&command) {
                    Ok(message) => web_sys::console::log_1(&format!("[debug] {}", message).into()),
                    Err(err) => {
                        web_sys::console::warn_1(&format!("[debug] {} -> {}", command, err).into())
                    }
                }
            }

            // Check for menu actions before updating game
            let (create_room, join_room, room_code) =
                state_ref.game.get_menu_action(&input_snapshot);

            // Handle network actions
            if create_room {
                if validate_title_name_for_room_action(&mut state_ref) {
                    let server = signaling_server_url();
                    let ice = ice_config();
                    let code = state_ref.network.create_room(&server, &ice);
                    // Store the room code so it displays in the UI
                    state_ref.game.room_code_input = code;
                    // Don't start game yet - wait for connection on title screen
                }
            } else if join_room {
                if validate_title_name_for_room_action(&mut state_ref) {
                    let server = signaling_server_url();
                    let ice = ice_config();
                    state_ref.network.join_room(&server, &room_code, &ice);
                    // Don't start game yet - wait for connection on title screen
                }
            }

            // Update network (safely - returns false on connection failure).
            // The network runs on a wall-clock frame counter, not the game's
            // tick counter: under the hidden-tab watchdog the game ticks ~1/s
            // and frame-based network timeouts would stretch 60x.
            let frame_count = network_clock_frame();
            let network_ok = state_ref.network.update(frame_count);
            let supernode_id = state_ref.network.supernode_id;
            let became_host =
                state_ref.network.is_host && state_ref.last_supernode_id != supernode_id;
            state_ref.last_supernode_id = supernode_id;

            // Multiplayer title → game: per-client only (join-anytime).
            // There is no party-wide wait for N players or host "Start"; we leave the title once
            // this machine's room is non-empty and signaling reports Connected or WaitingForPeers.
            // Others may still be joining or already in-game (late join handled in network layer).
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
            let in_multiplayer_room = !state_ref.network.room_code.is_empty()
                && (state_ref.network.state == NetworkState::Connected
                    || state_ref.network.state == NetworkState::WaitingForPeers);

            let multiplayer_remote_players = if in_multiplayer_room {
                Some(state_ref.network.remote_players.clone())
            } else {
                None
            };
            if in_multiplayer_room && state_ref.game.scene == Scene::Game {
                let remote_players = multiplayer_remote_players
                    .as_ref()
                    .expect("multiplayer snapshot must exist during gameplay");
                let incoming_inputs = state_ref.network.take_input_frames();
                if !incoming_inputs.is_empty() {
                    let mut formatted_inputs = Vec::with_capacity(incoming_inputs.len());
                    for (peer_id, input_frame) in incoming_inputs {
                        formatted_inputs.push((format!("{:?}", peer_id), input_frame));
                    }
                    state_ref.game.queue_remote_inputs(&formatted_inputs);
                }
                state_ref
                    .game
                    .update_remote_predictions(remote_players, frame_count);
                let predictions = state_ref.game.remote_predictions().clone();
                state_ref.network.apply_predicted_states(&predictions);
            }

            let prev_scene = state_ref.game.scene;
            // Sim catch-up: occluded/background tabs tick at ~1-2Hz (rAF stops,
            // the watchdog drives the loop) while wall time keeps moving. Run
            // extra sim steps to track real time so a backgrounded host does
            // not freeze the authoritative world for everyone else. Capped so
            // a long stall fast-forwards gradually instead of in one burst.
            let net_delta = frame_count.saturating_sub(state_ref.last_tick_net_frame);
            state_ref.last_tick_net_frame = frame_count;
            // Background/occluded tab detection: rAF has stalled and the
            // watchdog is driving us at ~1-2Hz. A throttled node makes a poor
            // world authority, so a throttled root hands the role to another
            // member (cascading until a foreground node holds it).
            if net_delta >= 20 {
                state_ref.throttled_ticks = state_ref.throttled_ticks.saturating_add(1);
            } else {
                state_ref.throttled_ticks = 0;
            }
            if state_ref.throttled_ticks >= 5
                && state_ref.network.is_host
                && state_ref.network.peer_count() > 0
            {
                state_ref.network.handoff_root(frame_count);
                state_ref.throttled_ticks = 0;
            }
            // Zero steps is a valid outcome: rAF fires at the display refresh
            // rate (120+Hz on ProMotion/gaming screens) while the sim is
            // paced at 60/s by the wall clock. Forcing a minimum of one step
            // per tick ran the whole game at 2x speed on those displays — and
            // in multiplayer made each client's enemy prediction overshoot
            // the authority's wall-clock sim, so every correction yanked
            // enemies backward.
            let sim_steps = if state_ref.game.scene == Scene::Game {
                net_delta.min(30)
            } else {
                1
            };
            let mut step_input = input_snapshot.clone();
            for step in 0..sim_steps {
                if in_multiplayer_room {
                    let is_host = state_ref.network.is_host;
                    state_ref.game.update_multiplayer(
                        &step_input,
                        multiplayer_remote_players
                            .as_ref()
                            .expect("multiplayer snapshot must exist during gameplay"),
                        is_host,
                    );
                } else {
                    state_ref.game.update(&step_input);
                }
                if step + 1 < sim_steps {
                    // Degrade edge-triggered inputs to holds for the
                    // fast-forwarded steps, exactly as real frames would.
                    step_input.end_frame();
                }
            }
            if sim_steps > 0 {
                // Keep edge-triggered presses pending across 0-step ticks so
                // a tap landing between wall-clock frames isn't dropped.
                state_ref.input.end_frame();
            }

            if prev_scene != Scene::Game
                && state_ref.game.scene == Scene::Game
                && in_multiplayer_room
            {
                state_ref.network.reset_stats();
            }

            if in_multiplayer_room && state_ref.game.scene == Scene::Game {
                state_ref.network.tick_playtime(true, sim_steps);
            }

            if in_multiplayer_room && state_ref.game.scene == Scene::Game && became_host {
                if let Some(wave_start) = state_ref.game.last_wave_start {
                    state_ref.network.send_wave_start(wave_start);
                }
                let enemy_sync = state_ref.game.create_enemy_sync();
                state_ref.network.send_enemy_sync(enemy_sync);
                let paid_obstacles = state_ref.game.paid_obstacles.clone();
                state_ref
                    .network
                    .send_paid_obstacles_to_all(&paid_obstacles);
                let paid_names = state_ref.network.paid_name_reservations_snapshot();
                state_ref.network.send_paid_names_to_all(&paid_names);
            }

            // Multiplayer sync logic (only during gameplay with network)
            if state_ref.game.scene == Scene::Game
                && (state_ref.network.state == NetworkState::Connected
                    || state_ref.network.state == NetworkState::WaitingForPeers)
            {
                // Wave start sync - broadcast wave spawns so all clients spawn identically
                if state_ref.network.is_host {
                    // Host: Broadcast wave start when a new wave spawns
                    if let Some(wave_start) = state_ref.game.take_pending_wave_start() {
                        state_ref.network.send_wave_start(wave_start);
                    }

                    // Host: resync authoritative state for new data-channel peers.
                    // Use tree broadcast (same paths as steady-state), not direct socket.send per peer:
                    // in sparse relay mode the root may have no DC to a leaf; unicast drops snapshots.
                    if state_ref.network.has_new_peers_needing_state() {
                        let _new_peers = state_ref.network.take_new_peers_needing_state();
                        if let Some(wave_start) = state_ref.game.last_wave_start {
                            state_ref.network.send_wave_start(wave_start);
                        }
                        let enemy_sync = state_ref.game.create_enemy_sync();
                        state_ref.network.send_enemy_sync(enemy_sync);
                        let paid_obstacles = state_ref.game.paid_obstacles.clone();
                        state_ref
                            .network
                            .send_paid_obstacles_to_all(&paid_obstacles);
                        let paid_names = state_ref.network.paid_name_reservations_snapshot();
                        state_ref.network.send_paid_names_to_all(&paid_names);
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
                let mut outgoing_kill_events: Vec<net::EnemyKill> = Vec::new();
                for (enemy_type, enemy_id) in kills {
                    state_ref.network.record_local_kill(enemy_type);
                    let killer_hash = state_ref.network.local_peer_hash.unwrap_or(0);
                    let event_id = make_event_id(
                        killer_hash,
                        enemy_type as u64,
                        enemy_id as u64,
                        state_ref.game.frame_count as u64,
                    );
                    outgoing_kill_events.push(net::EnemyKill {
                        enemy_type: enemy_type as u8,
                        enemy_id,
                        killer_x: player_pos.x,
                        killer_y: player_pos.y,
                        killer_hash,
                        event_id,
                    });
                }
                if !outgoing_kill_events.is_empty() {
                    state_ref.network.send_enemy_kills(outgoing_kill_events);
                }

                let (attack_attempts, attack_hits) = state_ref.game.take_pending_attack_stats();
                if attack_attempts > 0 {
                    state_ref
                        .network
                        .record_local_attack_attempts(attack_attempts);
                }
                if attack_hits > 0 {
                    state_ref.network.record_local_attack_hits(attack_hits);
                }

                let stats_sync_stride = match state_ref.network.relay_congestion_level() {
                    0 => 300,
                    1 => 450,
                    _ => 600,
                };
                if state_ref.game.frame_count % stats_sync_stride == 0 {
                    state_ref.network.send_player_stats_snapshot();
                }

                // Process enemy kills from other players (optimistic first)
                let optimistic_kills = state_ref.network.take_enemy_kills_optimistic();
                for kill in optimistic_kills {
                    if let Some(enemy_type) = net::EnemyType::from_u8(kill.enemy_type) {
                        state_ref.game.kill_enemy(enemy_type, kill.enemy_id);
                    }
                }
                let confirmed_kills = state_ref.network.take_enemy_kills_confirmed();
                for kill in confirmed_kills {
                    if let Some(enemy_type) = net::EnemyType::from_u8(kill.enemy_type) {
                        state_ref.game.kill_enemy(enemy_type, kill.enemy_id);
                        if let Some(local_hash) = state_ref.network.local_peer_hash {
                            if kill.killer_hash == local_hash {
                                continue;
                            }
                        }
                        if let Some(remote_id) =
                            state_ref.network.resolve_peer_hash(kill.killer_hash)
                        {
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
                    let event_id = make_event_id(
                        victim_hash,
                        death.killed_by_type as u64,
                        death.killed_by_id as u64,
                        state_ref.game.frame_count as u64,
                    );
                    death.victim_hash = victim_hash;
                    death.event_id = event_id;
                    state_ref.network.send_player_death(death);
                }

                // Process player deaths from other players (optimistic/confirmed)
                let _optimistic_deaths = state_ref.network.take_player_deaths_optimistic();
                let confirmed_deaths = state_ref.network.take_player_deaths_confirmed();
                for death in confirmed_deaths {
                    if let Some(local_hash) = state_ref.network.local_peer_hash {
                        if death.victim_hash == local_hash {
                            continue;
                        }
                    }
                    if let Some(remote_id) = state_ref.network.resolve_peer_hash(death.victim_hash)
                    {
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
                    state_ref
                        .game
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

                // Paid ability requests - build proof hash and send for verification
                let paid_ability_requests = state_ref.game.take_paid_ability_requests();
                for request in paid_ability_requests {
                    let ability = state_ref.network.build_paid_ability(
                        request.ability_type,
                        request.x,
                        request.y,
                        request.radius,
                        request.nonce,
                    );
                    if state_ref.network.room_code.is_empty() {
                        state_ref.game.apply_paid_ability(ability, true);
                        continue;
                    }
                    state_ref.game.store_paid_ability_candidate(ability);
                    if state_ref.network.is_host {
                        state_ref.network.send_paid_ability(ability);
                    } else {
                        state_ref.network.send_paid_ability_to_supernode(ability);
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
                        state_ref
                            .network
                            .record_paid_obstacle_confirmation(obstacle.proof_hash, peer_id);
                    }

                    let verified = state_ref.network.verify_paid_obstacle(&obstacle);
                    if verified {
                        if let Some(local_id) = state_ref.network.local_peer_id {
                            state_ref
                                .network
                                .record_paid_obstacle_confirmation(obstacle.proof_hash, local_id);
                        }
                        state_ref
                            .network
                            .send_paid_obstacle_ack(net::PaidObstacleAck {
                                proof_hash: obstacle.proof_hash,
                            });

                        let has_supernode_ack = state_ref
                            .network
                            .paid_obstacle_has_supernode_ack(obstacle.proof_hash);
                        if has_supernode_ack
                            && state_ref
                                .network
                                .paid_obstacle_confirmation_count(obstacle.proof_hash)
                                >= 2
                        {
                            state_ref.game.apply_paid_obstacle(obstacle);
                        } else {
                            state_ref.game.store_paid_obstacle_candidate(obstacle);
                        }
                    } else {
                        state_ref.network.mark_supernode_bad(frame_count);
                        state_ref.game.remove_paid_obstacle(obstacle.proof_hash);
                    }
                }

                let incoming_paid_abilities = state_ref.network.take_paid_abilities();
                for (sender, ability) in incoming_paid_abilities {
                    let from_supernode = state_ref.network.is_host
                        || sender == "sync"
                        || state_ref.network.supernode_id.is_none()
                        || state_ref.network.is_supernode_sender(&sender);
                    if !from_supernode {
                        continue;
                    }

                    if let Some(peer_id) = state_ref.network.resolve_peer_id(&sender) {
                        state_ref
                            .network
                            .record_paid_ability_confirmation(ability.proof_hash, peer_id);
                    }

                    let verified = state_ref.network.verify_paid_ability(&ability);
                    if verified {
                        if let Some(local_id) = state_ref.network.local_peer_id {
                            state_ref
                                .network
                                .record_paid_ability_confirmation(ability.proof_hash, local_id);
                        }
                        state_ref
                            .network
                            .send_paid_ability_ack(net::PaidAbilityAck {
                                proof_hash: ability.proof_hash,
                            });

                        let has_supernode_ack = state_ref
                            .network
                            .paid_ability_has_supernode_ack(ability.proof_hash);
                        if has_supernode_ack
                            && state_ref
                                .network
                                .paid_ability_confirmation_count(ability.proof_hash)
                                >= 2
                        {
                            let is_local = state_ref
                                .network
                                .resolve_peer_id(&sender)
                                .map(|id| Some(id) == state_ref.network.local_peer_id)
                                .unwrap_or(false);
                            state_ref.game.apply_paid_ability(ability, is_local);
                        } else {
                            state_ref.game.store_paid_ability_candidate(ability);
                        }
                    } else {
                        state_ref.network.mark_supernode_bad(frame_count);
                    }
                }

                let incoming_paid_names = state_ref.network.take_paid_names();
                for (sender, reservation) in incoming_paid_names {
                    let from_supernode = state_ref.network.is_host
                        || sender == "sync"
                        || state_ref.network.supernode_id.is_none()
                        || state_ref.network.is_supernode_sender(&sender);
                    if !from_supernode {
                        continue;
                    }

                    if let Some(peer_id) = state_ref.network.resolve_peer_id(&sender) {
                        state_ref
                            .network
                            .record_paid_name_confirmation(reservation.proof_hash, peer_id);
                    }

                    let verified = state_ref.network.verify_paid_name_reservation(&reservation);
                    if verified {
                        if let Some(local_id) = state_ref.network.local_peer_id {
                            state_ref
                                .network
                                .record_paid_name_confirmation(reservation.proof_hash, local_id);
                        }
                        state_ref.network.send_paid_name_ack(net::PaidNameAck {
                            proof_hash: reservation.proof_hash,
                        });

                        let has_supernode_ack = state_ref
                            .network
                            .paid_name_has_supernode_ack(reservation.proof_hash);
                        if has_supernode_ack
                            && state_ref
                                .network
                                .paid_name_confirmation_count(reservation.proof_hash)
                                >= 2
                        {
                            let requested_name = reservation.name_string();
                            if state_ref.network.apply_paid_name_reservation(reservation) {
                                persist_name_reservation_cache(&state_ref.network);
                                if state_ref.network.local_name_owner_hash()
                                    == reservation.owner_hash
                                {
                                    state_ref.network.set_player_name(&requested_name);
                                    state_ref.game.player_name = requested_name.clone();
                                    state_ref.network.broadcast_local_player_name();
                                    save_player_name(&state_ref.game.player_name);
                                    state_ref.game.push_chat_line(
                                        "System".to_string(),
                                        format!("Reserved name {}.", requested_name),
                                    );
                                } else if let Some(new_name) =
                                    state_ref.network.ensure_local_name_not_reserved_by_other()
                                {
                                    state_ref.game.player_name = new_name.clone();
                                    state_ref.network.broadcast_local_player_name();
                                    save_player_name(&state_ref.game.player_name);
                                    state_ref.game.push_chat_line(
                                        "System".to_string(),
                                        format!(
                                            "That name was reserved by another player. Switched to {}.",
                                            new_name
                                        ),
                                    );
                                }
                            }
                        } else {
                            state_ref.network.store_paid_name_candidate(reservation);
                        }
                    } else {
                        state_ref.network.mark_supernode_bad(frame_count);
                    }
                }

                let incoming_acks = state_ref.network.take_paid_obstacle_acks();
                for (sender, ack) in incoming_acks {
                    if let Some(peer_id) = state_ref.network.resolve_peer_id(&sender) {
                        state_ref
                            .network
                            .record_paid_obstacle_confirmation(ack.proof_hash, peer_id);
                    }
                }

                let incoming_ability_acks = state_ref.network.take_paid_ability_acks();
                for (sender, ack) in incoming_ability_acks {
                    if let Some(peer_id) = state_ref.network.resolve_peer_id(&sender) {
                        state_ref
                            .network
                            .record_paid_ability_confirmation(ack.proof_hash, peer_id);
                    }
                }

                let incoming_name_acks = state_ref.network.take_paid_name_acks();
                for (sender, ack) in incoming_name_acks {
                    if let Some(peer_id) = state_ref.network.resolve_peer_id(&sender) {
                        state_ref
                            .network
                            .record_paid_name_confirmation(ack.proof_hash, peer_id);
                    }
                }

                let pending_hashes = state_ref.game.pending_paid_obstacle_hashes();
                for hash in pending_hashes {
                    if state_ref.network.paid_obstacle_has_supernode_ack(hash)
                        && state_ref.network.paid_obstacle_confirmation_count(hash) >= 2
                    {
                        if let Some(obstacle) = state_ref.game.take_paid_obstacle_candidate(hash) {
                            state_ref.game.apply_paid_obstacle(obstacle);
                        }
                    }
                }

                let pending_ability_hashes = state_ref.game.pending_paid_ability_hashes();
                for hash in pending_ability_hashes {
                    if state_ref.network.paid_ability_has_supernode_ack(hash)
                        && state_ref.network.paid_ability_confirmation_count(hash) >= 2
                    {
                        if let Some(ability) = state_ref.game.take_paid_ability_candidate(hash) {
                            state_ref.game.apply_paid_ability(ability, true);
                        }
                    }
                }

                let pending_name_hashes = state_ref.network.pending_paid_name_hashes();
                for hash in pending_name_hashes {
                    if state_ref.network.paid_name_has_supernode_ack(hash)
                        && state_ref.network.paid_name_confirmation_count(hash) >= 2
                    {
                        if let Some(reservation) = state_ref.network.take_paid_name_candidate(hash)
                        {
                            let requested_name = reservation.name_string();
                            if state_ref.network.apply_paid_name_reservation(reservation) {
                                persist_name_reservation_cache(&state_ref.network);
                                if state_ref.network.local_name_owner_hash()
                                    == reservation.owner_hash
                                {
                                    state_ref.network.set_player_name(&requested_name);
                                    state_ref.game.player_name = requested_name.clone();
                                    state_ref.network.broadcast_local_player_name();
                                    save_player_name(&state_ref.game.player_name);
                                    state_ref.game.push_chat_line(
                                        "System".to_string(),
                                        format!("Reserved name {}.", requested_name),
                                    );
                                } else if let Some(new_name) =
                                    state_ref.network.ensure_local_name_not_reserved_by_other()
                                {
                                    state_ref.game.player_name = new_name.clone();
                                    state_ref.network.broadcast_local_player_name();
                                    save_player_name(&state_ref.game.player_name);
                                    state_ref.game.push_chat_line(
                                        "System".to_string(),
                                        format!(
                                            "That name was reserved by another player. Switched to {}.",
                                            new_name
                                        ),
                                    );
                                }
                            }
                        }
                    }
                }

                if state_ref.network.is_host {
                    let cannon_shots = state_ref.game.take_pending_cannon_shots();
                    for shot in cannon_shots {
                        state_ref.network.send_cannon_shot(shot);
                    }
                }
                let cannon_shots = state_ref.network.take_cannon_shots();
                for shot in cannon_shots {
                    state_ref.game.spawn_projectile_from_shot(shot);
                }

                let projectile_reflections = state_ref.game.take_pending_projectile_reflections();
                for reflection in projectile_reflections {
                    state_ref.network.send_projectile_reflection(reflection);
                }
                let incoming_reflections = state_ref.network.take_projectile_reflections();
                for reflection in incoming_reflections {
                    state_ref.game.apply_projectile_reflection(reflection);
                }

                // PER-AREA AUTHORITY MODEL:
                // Every node simulates all enemies locally (prediction). The
                // player nearest an area owns its enemies and broadcasts
                // corrections for them; the host covers unassigned areas and
                // remains sole authority for world events (waves, spawns).
                let congestion = state_ref.network.relay_congestion_level();
                let room_players = state_ref.network.peer_count() + 1;
                let low_fanout_room = room_players <= 8;
                let is_host = state_ref.network.is_host;

                let enemy_sync_stride: u32 = match congestion {
                    0 if low_fanout_room => 4,
                    0 => 5,
                    1 => 6,
                    _ => 8,
                };
                // Wave starts and explicit late-join snapshots introduce the
                // complete enemy set. Keep only a low-rate healing snapshot;
                // broadcasting the whole world twice a second dominates room
                // bandwidth at scale.
                let host_enemy_intro_stride: u32 = 600;
                // Delta-based, never modulo: the wall-clock frame counter
                // advances in fixed jumps on throttled tabs, which makes
                // `% stride` lock onto a non-zero residue forever.
                // A watchdog-driven (hidden/occluded) tab simulates in coarse
                // catch-up bursts; broadcasting corrections from that sim
                // teleports enemies on every healthy screen. Stay silent while
                // throttled — the root reassigns our areas within frames, and
                // receivers' prediction covers the gap.
                let sim_throttled = state_ref.throttled_ticks > 0;
                if frame_count.saturating_sub(state_ref.last_enemy_sync_sent_frame)
                    >= enemy_sync_stride
                {
                    state_ref.last_enemy_sync_sent_frame = frame_count;
                    let owned = state_ref.network.owned_area_ids();
                    if !sim_throttled && (is_host || !owned.is_empty()) {
                        let assigned = state_ref.network.assigned_area_ids();
                        let enemy_sync = state_ref
                            .game
                            .create_enemy_sync_in_areas(&owned, &assigned, is_host);
                        if !enemy_sync.enemies.is_empty() {
                            state_ref.network.send_enemy_sync(enemy_sync);
                        }
                    }
                }
                if is_host && !sim_throttled {
                    let intro_due = state_ref.last_enemy_intro_wave != state_ref.game.wave
                        || state_ref.last_enemy_intro_sent_frame == 0
                        || frame_count.saturating_sub(state_ref.last_enemy_intro_sent_frame)
                            >= host_enemy_intro_stride;
                    if intro_due {
                        let enemy_intro_sync = state_ref.game.create_enemy_sync();
                        if !enemy_intro_sync.enemies.is_empty() {
                            state_ref.last_enemy_intro_sent_frame = frame_count;
                            state_ref.last_enemy_intro_wave = state_ref.game.wave;
                            state_ref.network.send_enemy_sync(enemy_intro_sync);
                        }
                    }
                }
                // Everyone applies incoming corrections (already filtered to
                // areas their origin legitimately owns).
                for pending_sync in state_ref.network.take_enemy_syncs() {
                    state_ref.game.apply_enemy_sync(
                        &pending_sync.sync,
                        pending_sync.origin_hash,
                        pending_sync.from_host,
                        pending_sync.introductions_only,
                    );
                    if !is_host {
                        state_ref.game.clear_respawn_sync();
                    }
                    if pending_sync.from_host {
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

            // Send player/input updates with adaptive cadence.
            let congestion = state_ref.network.relay_congestion_level();
            let room_players = state_ref.network.peer_count() + 1;
            let low_fanout_room = room_players <= 8;
            let very_small_room = room_players <= 4;
            let player_send_stride = match congestion {
                0 if low_fanout_room => 2,
                0 => 3,
                1 if low_fanout_room => 3,
                1 => 4,
                _ => 6,
            };
            // Strides are denominated in 60/s sim frames; advancing by sim
            // steps (not browser callbacks) keeps the wall-clock send rate
            // independent of the display refresh rate.
            state_ref.send_counter += sim_steps;
            if state_ref.send_counter >= player_send_stride {
                state_ref.send_counter = 0;
                // Stamp with the sim frame counter — the SAME domain as
                // InputFrame stamps — so receivers can anchor input-replay
                // predictions at this state and match our input frames
                // against it. (The wall-clock net frame is a different
                // counter with a different epoch; mixing the two made input
                // replay never match and froze prediction anchors.)
                let sim_frame = state_ref.game.frame_count;
                let player = &state_ref.game.player;
                let player_state = PlayerState::new(
                    sim_frame,
                    player.pos,
                    player.look_dir,
                    player.move_dir,
                    player.alive,
                    player.is_attacking(),
                    player.blocking,
                    player.is_phasing(),
                    player.is_shielded(),
                );
                state_ref.network.send_player_state(player_state);
            }

            if in_multiplayer_room && state_ref.game.scene == Scene::Game {
                let input_send_stride = match congestion {
                    0 if very_small_room => 1,
                    0 => 2,
                    1 => 3,
                    _ => 4,
                };
                state_ref.input_send_counter += sim_steps;
                if state_ref.input_send_counter >= input_send_stride {
                    state_ref.input_send_counter = 0;
                    let frame = state_ref.game.frame_count;
                    let input_frame = net::InputFrame {
                        frame,
                        input: input_snapshot.get_raw(),
                    };
                    state_ref.network.send_input_frame(input_frame);
                }
            }

            if in_multiplayer_room {
                state_ref.network.flush_relay_batches();
            }
        }
        {
            let state_ref = state.borrow();
            state_ref
                .renderer
                .render(&state_ref.game, &state_ref.network);
        }

        // Queue exactly one upcoming rAF. The shim clears the flag when the
        // browser actually fires it; watchdog-driven ticks leave it queued.
        if !RAF_PENDING.with(|cell| cell.replace(true)) {
            window_clone
                .request_animation_frame(
                    RAF_SHIM
                        .with(|shim| {
                            shim.borrow()
                                .as_ref()
                                .map(|s| s.as_ref().unchecked_ref::<js_sys::Function>().clone())
                        })
                        .expect("raf shim not installed")
                        .unchecked_ref(),
                )
                .expect("failed to request animation frame");
        }
    }));

    // The shim is what rAF actually invokes: it clears the pending flag and
    // runs the tick. The watchdog calls the tick directly (flag untouched).
    let tick_for_shim = f.clone();
    RAF_SHIM.with(|shim| {
        *shim.borrow_mut() = Some(Closure::new(move || {
            RAF_PENDING.with(|cell| cell.set(false));
            if let Some(tick) = tick_for_shim.borrow().as_ref() {
                let func: &js_sys::Function = tick.as_ref().unchecked_ref();
                let _ = func.call0(&JsValue::NULL);
            }
        }));
    });

    // Hidden-tab watchdog: browsers stop firing rAF entirely for occluded or
    // backgrounded pages, which used to freeze the whole client (no network
    // pump, no heartbeats -> the player "drops out" and gets pruned, and a
    // dead root could never be replaced). Timers still run at >=1Hz in hidden
    // tabs (WebRTC-active pages are exempt from intensive throttling), so a
    // stalled loop keeps ticking here in slow motion until rAF resumes.
    let tick_for_watchdog = f.clone();
    let watchdog = Closure::<dyn FnMut()>::new(move || {
        let stalled = LAST_TICK_MS.with(|cell| js_sys::Date::now() - cell.get()) > 700.0;
        if stalled {
            if let Some(tick) = tick_for_watchdog.borrow().as_ref() {
                let func: &js_sys::Function = tick.as_ref().unchecked_ref();
                let _ = func.call0(&JsValue::NULL);
            }
        }
    });
    window
        .set_interval_with_callback_and_timeout_and_arguments_0(
            watchdog.as_ref().unchecked_ref(),
            500,
        )
        .expect("failed to install loop watchdog");
    watchdog.forget();

    RAF_PENDING.with(|cell| cell.set(true));
    window
        .request_animation_frame(
            RAF_SHIM
                .with(|shim| {
                    shim.borrow()
                        .as_ref()
                        .map(|s| s.as_ref().unchecked_ref::<js_sys::Function>().clone())
                })
                .expect("raf shim not installed")
                .unchecked_ref(),
        )
        .expect("failed to request animation frame");

    Ok(())
}

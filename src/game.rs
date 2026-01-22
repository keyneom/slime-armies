use crate::input::Input;
use crate::entities::{Player, ExplosionPool, Spider, Cannon, Snake, ProjectilePool, CannonEvent};
use crate::math::Vec2;
use crate::net::PlayerState;
use crate::world::{Camera, ChunkManager};
use rand::{SeedableRng, Rng};
use rand_xoshiro::Xoshiro256PlusPlus;
use js_sys;
use std::collections::{HashMap, HashSet};

// Camera zoom (>1.0 zooms in to show less world)
const CAMERA_ZOOM: f32 = 1.0;
// Creature scale for collision/interaction sizes
const CREATURE_SCALE: f32 = 2.0;
pub const MAP_OVERLAY_SIZE: f32 = 520.0;
pub const MAP_OVERLAY_PADDING: f32 = 40.0;
const MAP_ZOOM_MIN: f32 = 0.25;
const MAP_ZOOM_MAX: f32 = 4.0;
const MAP_ZOOM_STEP: f32 = 1.25;
const ROLLBACK_WINDOW_FRAMES: u32 = 6;
const SPAWN_SAFE_DISTANCE: f32 = 90.0;
const SPAWN_CHUNK_COOLDOWN_FRAMES: u32 = 300;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Scene {
    Title,      // Main menu with options
    Game,
    GameOver,
}

/// Sound events that can be triggered during gameplay
#[derive(Debug, Clone, Copy)]
pub enum SoundEvent {
    Attack,
    Block,
    Deflect,
    Phase,
    EnemyKill,
    Hit,
    Death,
    Explosion,
    MenuSelect,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MenuSelection {
    Play,
    CreateRoom,
}

impl MenuSelection {
    pub fn next(self) -> Self {
        match self {
            MenuSelection::Play => MenuSelection::CreateRoom,
            MenuSelection::CreateRoom => MenuSelection::Play,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            MenuSelection::Play => MenuSelection::CreateRoom,
            MenuSelection::CreateRoom => MenuSelection::Play,
        }
    }
}

pub struct Game {
    pub scene: Scene,
    pub player: Player,
    pub spiders: Vec<Spider>,
    pub cannons: Vec<Cannon>,
    pub snakes: Vec<Snake>,
    pub projectiles: ProjectilePool,
    pub explosions: ExplosionPool,
    pub camera: Camera,
    pub chunks: ChunkManager,
    pub frame_count: u32,
    pub start_frame: u32,
    pub end_frame: u32,
    pub wave: u32,
    pub kills: u32,
    pub deaths: u32,
    pub width: u32,
    pub height: u32,
    pub world_seed: u64,
    pub explored_chunks: HashSet<(i32, i32)>,
    pub map_open: bool,
    pub map_zoom: f32,
    pub map_center: Vec2,
    pub map_target: Option<Vec2>,
    pub map_text_input_active: bool,
    pub map_active_field: u8, // 0 = X, 1 = Y
    pub map_input_x: String,
    pub map_input_y: String,
    pub paid_obstacles: Vec<crate::net::PaidObstacle>,
    paid_obstacle_hashes: HashSet<[u8; 32]>,
    paid_obstacle_map: HashMap<[u8; 32], crate::world::Obstacle>,
    pending_paid_obstacle_candidates: HashMap<[u8; 32], crate::net::PaidObstacle>,
    pub pending_paid_obstacles: Vec<crate::net::PaidObstacle>,
    pub pending_cannon_shots: Vec<crate::net::CannonShot>,
    pub queued_join_room: bool,
    pub input_history: Vec<crate::net::InputFrame>,
    remote_simulations: HashMap<String, RemoteSimulation>,
    remote_predictions: HashMap<String, PlayerState>,
    pub wave_kill_counts: [u32; 3],
    pub wave_kill_targets: [u32; 3],
    spawn_chunk_last_frame: HashMap<(i32, i32), u32>,
    pub player_list_open: bool,
    pub player_list_scroll: i32,
    pub player_list_sort: u8,
    pub player_list_sort_asc: bool,
    pub player_list_search: String,
    pub player_list_search_active: bool,
    pub chat_open: bool,
    pub chat_input: String,
    pub chat_log: Vec<ChatLine>,
    pub last_chat_send_frame: u32,
    pub mobile_mode: bool,
    spawn_progress: f32,
    last_spawn_positions: Vec<Vec2>,
    rng: Xoshiro256PlusPlus,
    // Menu state
    pub menu_selection: MenuSelection,
    pub player_name: String,
    pub room_code_input: String,
    pub text_input_active: bool,  // True when typing in a text field
    pub active_input_field: u8,   // 0 = name, 1 = room code
    // Sound events to be played
    pub sound_events: Vec<SoundEvent>,
    /// Enemy kills that need to be reported to network (for non-host clients)
    pub pending_enemy_kills: Vec<(crate::net::EnemyType, u16)>,
    /// Player deaths that need to be reported to network
    pub pending_player_deaths: Vec<crate::net::PlayerDeath>,
    /// Pending wave start to broadcast (host only)
    pub pending_wave_start: Option<crate::net::WaveStart>,
    /// Whether we're waiting for wave start from host (client only)
    pub waiting_for_wave_start: bool,
    /// Last wave start info (for late joiners)
    pub last_wave_start: Option<crate::net::WaveStart>,
    /// Pending respawn request (needs enemy sync from host)
    pub pending_respawn: bool,
    /// Snapshot of enemy positions for consistent rendering (updated at sync rate)
    /// This ensures host and client see enemies at the same visual update rate
    pub enemy_render_snapshot: Option<EnemyRenderSnapshot>,
}

/// Snapshot of enemy positions for rendering (separate from simulation)
#[derive(Clone)]
pub struct EnemyRenderSnapshot {
    pub spider_positions: Vec<(Vec2, Vec2, bool)>,  // (pos, dir, alive)
    pub cannon_positions: Vec<(Vec2, Vec2, Vec2, bool)>,  // (pos, dir, look_dir, alive)
    pub snake_positions: Vec<(Vec2, Vec2, f32, bool)>,  // (pos, dir, size, alive)
}

pub struct ChatLine {
    pub name: String,
    pub text: String,
    pub frame: u32,
}

struct RemoteSimulation {
    player: Player,
    last_sim_frame: u32,
    last_authoritative_frame: u32,
    last_input: u8,
    pending_inputs: Vec<crate::net::InputFrame>,
    snapshots: std::collections::VecDeque<(u32, Player, u8)>,
    rollback_from: Option<u32>,
}

impl Game {
    pub fn new(width: u32, height: u32) -> Self {
        let world_seed = 12345; // Fixed seed for consistent world generation
        Self {
            scene: Scene::Title,
            player: Player::new_at_position(Vec2::ZERO), // Start at world origin
            spiders: Vec::new(),
            cannons: Vec::new(),
            snakes: Vec::new(),
            projectiles: ProjectilePool::new(20),
            explosions: ExplosionPool::new(20),
            camera: Camera::new(width, height, CAMERA_ZOOM),
            chunks: ChunkManager::new(world_seed),
            frame_count: 0,
            start_frame: 0,
            end_frame: 0,
            wave: 0,
            kills: 0,
            deaths: 0,
            width,
            height,
            world_seed,
            explored_chunks: HashSet::new(),
            map_open: false,
            map_zoom: 1.0,
            map_center: Vec2::ZERO,
            map_target: None,
            map_text_input_active: false,
            map_active_field: 0,
            map_input_x: String::new(),
            map_input_y: String::new(),
            paid_obstacles: Vec::new(),
            paid_obstacle_hashes: HashSet::new(),
            paid_obstacle_map: HashMap::new(),
            pending_paid_obstacle_candidates: HashMap::new(),
            pending_paid_obstacles: Vec::new(),
            pending_cannon_shots: Vec::new(),
            queued_join_room: false,
            input_history: Vec::new(),
            remote_simulations: HashMap::new(),
            remote_predictions: HashMap::new(),
            wave_kill_counts: [0; 3],
            wave_kill_targets: [0; 3],
            spawn_chunk_last_frame: HashMap::new(),
            player_list_open: false,
            player_list_scroll: 0,
            player_list_sort: 0,
            player_list_sort_asc: false,
            player_list_search: String::new(),
            player_list_search_active: false,
            chat_open: false,
            chat_input: String::new(),
            chat_log: Vec::new(),
            last_chat_send_frame: u32::MAX,
            mobile_mode: false,
            spawn_progress: 0.0,
            last_spawn_positions: Vec::new(),
            rng: Xoshiro256PlusPlus::seed_from_u64(0),
            // Menu state
            menu_selection: MenuSelection::Play,
            player_name: Self::generate_default_name(),
            room_code_input: String::new(),
            text_input_active: false,
            active_input_field: 0,
            sound_events: Vec::new(),
            pending_enemy_kills: Vec::new(),
            pending_player_deaths: Vec::new(),
            pending_wave_start: None,
            waiting_for_wave_start: false,
            last_wave_start: None,
            pending_respawn: false,
            enemy_render_snapshot: None,
        }
    }

    pub fn queue_remote_inputs(&mut self, inputs: &[(String, crate::net::InputFrame)]) {
        for (peer_id, input_frame) in inputs {
            let sim = self.remote_simulations.entry(peer_id.clone()).or_insert_with(|| {
                RemoteSimulation::new_placeholder(self.frame_count)
            });
            sim.queue_input(input_frame.frame, input_frame.input);
        }
    }

    pub fn set_mobile_mode(&mut self, enabled: bool) {
        self.mobile_mode = enabled;
    }

    pub fn update_remote_predictions(&mut self, remote_players: &HashMap<String, crate::net::RemotePlayer>) {
        self.remote_predictions.clear();

        for (peer_id, remote) in remote_players {
            let sim = self.remote_simulations.entry(peer_id.clone()).or_insert_with(|| {
                let state = PlayerState::new(
                    remote.pos,
                    remote.look_dir,
                    remote.move_dir,
                    remote.alive,
                    remote.attacking,
                    remote.blocking,
                    remote.phasing,
                );
                RemoteSimulation::new_from_state(&state, remote.last_update_frame())
            });

            let authoritative_frame = remote.last_update_frame();
            if authoritative_frame > sim.last_authoritative_frame {
                let state = PlayerState::new(
                    remote.pos,
                    remote.look_dir,
                    remote.move_dir,
                    remote.alive,
                    remote.attacking,
                    remote.blocking,
                    remote.phasing,
                );
                sim.apply_authoritative_state(&state, authoritative_frame);
            }

            sim.simulate_to(self.frame_count, &self.chunks);
            self.remote_predictions.insert(peer_id.clone(), sim.predicted_state());
        }

        self.remote_simulations.retain(|peer_id, _| remote_players.contains_key(peer_id));
    }

    pub fn remote_predictions(&self) -> &HashMap<String, PlayerState> {
        &self.remote_predictions
    }

    /// Create a snapshot of current enemy positions for rendering
    /// Called at sync rate (every 6 frames) for consistent visuals between host and client
    pub fn snapshot_enemies_for_render(&mut self) {
        let spider_positions: Vec<_> = self.spiders.iter()
            .map(|s| (s.pos, s.dir, s.alive))
            .collect();
        let cannon_positions: Vec<_> = self.cannons.iter()
            .map(|c| (c.pos, c.dir, c.look_dir, c.alive))
            .collect();
        let snake_positions: Vec<_> = self.snakes.iter()
            .map(|s| (s.pos, s.dir, s.size, s.alive))
            .collect();

        self.enemy_render_snapshot = Some(EnemyRenderSnapshot {
            spider_positions,
            cannon_positions,
            snake_positions,
        });
    }

    fn generate_default_name() -> String {
        let adjectives = ["SWIFT", "BRAVE", "SLY", "BOLD", "KEEN", "WILD", "COOL", "RAD"];
        let nouns = ["SLIME", "BLOB", "GOO", "OOZE", "JELLY", "GLOB", "PUDDLE", "DROP"];

        let now = js_sys::Date::now() as u64;
        let adj_idx = (now % adjectives.len() as u64) as usize;
        let noun_idx = ((now / 8) % nouns.len() as u64) as usize;
        let num = now % 1000;

        format!("{}{}{}", adjectives[adj_idx], nouns[noun_idx], num)
    }

    /// Handle character input from keyboard
    pub fn handle_char_input(&mut self, c: char) {
        if !self.text_input_active || self.scene != Scene::Title {
            return;
        }

        let max_len = if self.active_input_field == 0 { 20 } else { 6 };
        let input = if self.active_input_field == 0 {
            &mut self.player_name
        } else {
            &mut self.room_code_input
        };

        if c.is_alphanumeric() && input.len() < max_len {
            // Always uppercase for consistency
            input.push(c.to_ascii_uppercase());
        }
    }

    /// Handle backspace
    pub fn handle_backspace(&mut self) {
        if !self.text_input_active || self.scene != Scene::Title {
            return;
        }

        let input = if self.active_input_field == 0 {
            &mut self.player_name
        } else {
            &mut self.room_code_input
        };

        input.pop();
    }

    /// Check if text input is currently active
    pub fn is_text_input_active(&self) -> bool {
        self.text_input_active && self.scene == Scene::Title
    }

    pub fn update(&mut self, input: &Input) {
        self.frame_count += 1;
        // Clear sound events from previous frame
        self.sound_events.clear();
        self.prune_chat_log();

        match self.scene {
            Scene::Title => self.update_title(input),
            Scene::Game => self.update_game(input),
            Scene::GameOver => self.update_gameover(input),
        }
    }

    /// Update with multiplayer support - enemies target closest of all players
    pub fn update_multiplayer(&mut self, input: &Input, remote_players: &std::collections::HashMap<String, crate::net::RemotePlayer>, is_host: bool) {
        self.frame_count += 1;
        // Clear sound events from previous frame
        self.sound_events.clear();
        self.prune_chat_log();

        match self.scene {
            Scene::Title => self.update_title(input),
            Scene::Game => self.update_game_multiplayer(input, remote_players, is_host),
            Scene::GameOver => self.update_gameover(input),
        }
    }

    fn update_title(&mut self, input: &Input) {
        use crate::input::{BUTTON_ATTACK, BUTTON_UP, BUTTON_DOWN, BUTTON_LEFT, BUTTON_RIGHT};

        // If text input is active, don't process menu navigation
        if self.text_input_active {
            // Escape/Attack exits text input
            if input.is_pressed(BUTTON_ATTACK) {
                self.text_input_active = false;
            }
            return;
        }

        // Menu navigation
        if input.is_pressed(BUTTON_UP) {
            self.menu_selection = self.menu_selection.prev();
            self.sound_events.push(SoundEvent::MenuSelect);
        }
        if input.is_pressed(BUTTON_DOWN) {
            self.menu_selection = self.menu_selection.next();
            self.sound_events.push(SoundEvent::MenuSelect);
        }

        // Tab between name and room code input
        if input.is_pressed(BUTTON_LEFT) || input.is_pressed(BUTTON_RIGHT) {
            self.active_input_field = if self.active_input_field == 0 { 1 } else { 0 };
        }

        // Select menu item with space
        if input.is_pressed(BUTTON_ATTACK) {
            match self.menu_selection {
                MenuSelection::Play => {
                    self.start_game();
                }
                MenuSelection::CreateRoom => {
                    // Will be handled by lib.rs to create room
                    // For now just mark that we want to create
                }
            }
        }
    }

    /// Called to indicate user wants to edit text field
    pub fn activate_text_input(&mut self, field: u8) {
        self.text_input_active = true;
        self.active_input_field = field;
    }

    /// Get menu action - returns (should_create_room, should_join_room, room_code)
    pub fn get_menu_action(&mut self, input: &Input) -> (bool, bool, String) {
        use crate::input::BUTTON_ATTACK;

        if self.text_input_active || self.scene != Scene::Title {
            if self.queued_join_room && self.room_code_input.len() >= 4 {
                self.queued_join_room = false;
                return (false, true, self.room_code_input.clone());
            }
            return (false, false, String::new());
        }

        if self.queued_join_room && self.room_code_input.len() >= 4 {
            self.queued_join_room = false;
            return (false, true, self.room_code_input.clone());
        }

        if input.is_pressed(BUTTON_ATTACK) {
            match self.menu_selection {
                MenuSelection::Play => {
                    // Just start game
                }
                MenuSelection::CreateRoom => {
                    return (true, false, String::new());
                }
            }
        }

        (false, false, String::new())
    }

    fn update_game(&mut self, input: &Input) {
        // Single-player update - all enemies target local player
        self.update_game_with_targets(input, &[self.player.pos], true, 1);
    }

    /// Update game with multiplayer support
    /// AUTHORITATIVE HOST MODEL:
    /// - Host runs enemy AI and sends positions to clients
    /// - Clients do NOT run enemy AI, they receive positions from host
    /// - All players run local collision detection for responsive gameplay
    pub fn update_game_multiplayer(&mut self, input: &Input, remote_players: &std::collections::HashMap<String, crate::net::RemotePlayer>, is_host: bool) {
        // Build list of all player positions for enemy targeting (host uses this for AI)
        let mut target_positions = vec![self.player.pos];
        let mut remote_ids: Vec<_> = remote_players.keys().collect();
        remote_ids.sort();
        for peer_id in remote_ids {
            let remote = &remote_players[peer_id];
            if remote.alive {
                let pos = self
                    .remote_predictions
                    .get(peer_id)
                    .map(|state| state.pos())
                    .unwrap_or(remote.pos);
                target_positions.push(pos);
            }
        }

        // Player count includes self + all remote players (alive or dead, they're still in the game)
        let player_count = 1 + remote_players.len();

        // Host runs enemy AI (authoritative simulation)
        // Clients do NOT run enemy AI - they receive positions from host via enemy sync
        let run_enemy_ai = is_host;
        self.update_game_with_targets(input, &target_positions, run_enemy_ai, player_count);
    }

    fn update_game_with_targets(&mut self, input: &Input, target_positions: &[Vec2], run_enemy_ai: bool, player_count: usize) {
        let player_pos = self.player.pos;
        let player_look_dir = self.player.look_dir;
        let frame_count = self.frame_count;

            if input.is_pressed(crate::input::BUTTON_MAP) {
                self.toggle_map();
            }

        // Capture input history for future rollback
        self.input_history.push(crate::net::InputFrame {
            frame: self.frame_count,
            input: input.get_raw(),
        });
        self.input_history.retain(|entry| self.frame_count.saturating_sub(entry.frame) <= ROLLBACK_WINDOW_FRAMES);

        // Update chunks around all active players (host uses all for enemy AI)
        self.chunks.update_for_positions(target_positions);

        if run_enemy_ai && self.wave == 0 && self.player.alive {
            self.spawn_wave_for_players(player_count);
        }

        for pos in target_positions {
            let cx = (pos.x / crate::world::CHUNK_SIZE as f32).floor() as i32;
            let cy = (pos.y / crate::world::CHUNK_SIZE as f32).floor() as i32;
            self.explored_chunks.insert((cx, cy));
        }

        if run_enemy_ai && self.wave > 0 {
            if self.last_spawn_positions.len() != target_positions.len() {
                self.last_spawn_positions = target_positions.to_vec();
            }
            let mut moved_total = 0.0;
            for (idx, pos) in target_positions.iter().enumerate() {
                if let Some(prev) = self.last_spawn_positions.get(idx) {
                    moved_total += pos.distance(*prev);
                }
            }
            self.last_spawn_positions = target_positions.to_vec();
            self.spawn_progress += moved_total;

            let spawn_stride = self.width as f32;
            while self.spawn_progress >= spawn_stride {
                self.spawn_enemies_for_movement(target_positions, player_count, frame_count);
                self.spawn_progress -= spawn_stride;
            }
        }

        // Capture player state before update for sound triggers
        let was_attacking = self.player.is_attacking();
        let was_phasing = self.player.is_phasing();

        if self.map_open {
            self.update_map_controls(input);
        } else {
            // Update player
            if self.player.alive {
                self.player.update_infinite(input, &self.chunks);

                // Sound for attack start (transition from not attacking to attacking)
                if !was_attacking && self.player.is_attacking() {
                    self.sound_events.push(SoundEvent::Attack);
                }
                // Sound for phase start (transition from not phasing to phasing)
                if !was_phasing && self.player.is_phasing() {
                    self.sound_events.push(SoundEvent::Phase);
                }
            } else if self.frame_count - self.end_frame > 120 {
                self.scene = Scene::GameOver;
            }
        }

        // Update camera to follow player
        self.camera.follow(self.player.pos);

        // Get visible bounds (for future culling optimizations)
        let _visible_bounds = self.camera.visible_bounds();

        // Collect actions to avoid borrow conflicts
        let mut player_killed = false;
        let mut player_killed_by: Option<(u8, u16)> = None;
        let mut killed_spiders: Vec<usize> = Vec::new();
        let mut killed_cannons: Vec<usize> = Vec::new();
        let mut killed_snakes: Vec<usize> = Vec::new();
        let mut new_projectiles: Vec<(Vec2, Vec2, i32)> = Vec::new();
        let mut spider_bumps: Vec<(usize, Vec2, f32)> = Vec::new();
        let mut cannon_bumps: Vec<(usize, Vec2, f32)> = Vec::new();
        let mut snake_bumps: Vec<(usize, Vec2, f32)> = Vec::new();

        // Update spiders and collect collisions (with obstacle awareness)
        for (i, spider) in self.spiders.iter_mut().enumerate() {
            if spider.alive {
                // Only run enemy AI if we're the host (or single player)
                if run_enemy_ai {
                    // Find closest player for this enemy
                    let target = Self::find_closest_target(spider.pos, target_positions);
                    spider.update_infinite(target, &self.chunks);
                }

                if self.player.collide_attack(spider.pos, spider.radius() * CREATURE_SCALE) {
                    killed_spiders.push(i);
                } else if self.player.collide_block(spider.pos, (spider.radius() - 3.5) * CREATURE_SCALE) {
                    let distance = player_pos.distance(spider.pos);
                    let bump = (11.0 * CREATURE_SCALE - distance).max(1.5 * CREATURE_SCALE);
                    spider_bumps.push((i, player_look_dir, bump));
                } else if self.player.collide_body(spider.pos, (spider.radius() - 3.5) * CREATURE_SCALE) {
                    player_killed = true;
                    if player_killed_by.is_none() {
                        player_killed_by = Some((0, spider.id as u16));
                    }
                }
            }
        }

        let half_w = self.camera.screen_width / (2.0 * self.camera.zoom);
        let half_h = self.camera.screen_height / (2.0 * self.camera.zoom);

        // Update cannons and collect collisions (with obstacle awareness)
        for (i, cannon) in self.cannons.iter_mut().enumerate() {
            if cannon.alive {
                if run_enemy_ai {
                    // Cannons shoot when on screen for any player camera
                    let on_screen = target_positions.iter().any(|target| {
                        cannon.pos.x + 50.0 >= target.x - half_w
                            && cannon.pos.x - 50.0 <= target.x + half_w
                            && cannon.pos.y + 50.0 >= target.y - half_h
                            && cannon.pos.y - 50.0 <= target.y + half_h
                    });
                    let target = Self::find_closest_target(cannon.pos, target_positions);
                    if let Some(event) = cannon.update_infinite(target, frame_count, on_screen, &self.chunks) {
                        match event {
                            CannonEvent::Shoot { pos, speed } => {
                                new_projectiles.push((pos, speed, 80));
                                self.pending_cannon_shots.push(crate::net::CannonShot {
                                    x: pos.x,
                                    y: pos.y,
                                    vx: speed.x,
                                    vy: speed.y,
                                });
                            }
                        }
                    }
                }

                if self.player.collide_attack(cannon.pos, cannon.radius()) {
                    killed_cannons.push(i);
                } else if self.player.collide_block(cannon.pos, cannon.radius() - 4.0) {
                    let distance = player_pos.distance(cannon.pos);
                    let bump = (11.5 - distance).max(1.5);
                    cannon_bumps.push((i, player_look_dir, bump));
                } else if self.player.collide_body(cannon.pos, cannon.radius() - 4.0) {
                    player_killed = true;
                    if player_killed_by.is_none() {
                        player_killed_by = Some((1, cannon.id as u16));
                    }
                }
            }
        }

        // Update snakes (back to front for segment following)
        if run_enemy_ai {
            for i in (0..self.snakes.len()).rev() {
                if self.snakes[i].alive {
                    let previous = if i > 0 && self.snakes[i - 1].alive {
                        Some(self.snakes[i - 1].clone())
                    } else {
                        None
                    };
                    // Snake head follows closest player, segments follow each other
                    let target = if previous.is_none() {
                        Self::find_closest_target(self.snakes[i].pos, target_positions)
                    } else {
                        player_pos // segments follow previous, target is only used if no previous
                    };
                    self.snakes[i].update(target, previous.as_ref());
                }
            }
        }

        // Check snake collisions separately
        for (i, snake) in self.snakes.iter().enumerate() {
            if snake.alive {
                if self.player.collide_attack(snake.pos, snake.radius() * CREATURE_SCALE) {
                    killed_snakes.push(i);
                } else if self.player.collide_block(snake.pos, snake.radius() * CREATURE_SCALE) {
                    let distance = player_pos.distance(snake.pos);
                    let bump = (8.5 * CREATURE_SCALE + snake.radius() * CREATURE_SCALE - distance)
                        .max(1.5 * CREATURE_SCALE)
                        / 2.0;
                    snake_bumps.push((i, player_look_dir, bump));
                    self.player.pos -= player_look_dir * bump;
                } else if self.player.collide_body(snake.pos, snake.radius() * CREATURE_SCALE) {
                    player_killed = true;
                    if player_killed_by.is_none() {
                        player_killed_by = Some((2, snake.id as u16));
                    }
                }
            }
        }

        // Update projectiles (with obstacle collision)
        self.projectiles.update_with_collision(&self.chunks);

        // Collect projectile actions
        let mut projectiles_to_kill: Vec<usize> = Vec::new();
        let mut projectiles_to_reflect: Vec<usize> = Vec::new();

        // Check projectile collisions
        for (idx, projectile) in self.projectiles.projectiles.iter().enumerate() {
            if !projectile.alive {
                continue;
            }
            if projectile.hostile {
                // Original uses radius 3 for attack/block, radius 1 for body
                if self.player.collide_attack(projectile.pos, 3.0 * CREATURE_SCALE) {
                    projectiles_to_kill.push(idx);
                } else if self.player.collide_block(projectile.pos, 3.0 * CREATURE_SCALE) {
                    projectiles_to_reflect.push(idx);
                } else if self.player.collide_body(projectile.pos, 1.0 * CREATURE_SCALE) {
                    player_killed = true;
                    if player_killed_by.is_none() {
                        player_killed_by = Some((3, 0));
                    }
                }
            } else {
                // Reflected projectiles can kill enemies
                for (i, spider) in self.spiders.iter().enumerate() {
                    if spider.alive && projectile.pos.distance(spider.pos) < 5.0 * CREATURE_SCALE && !killed_spiders.contains(&i) {
                        killed_spiders.push(i);
                    }
                }
                for (i, cannon) in self.cannons.iter().enumerate() {
                    if cannon.alive && projectile.pos.distance(cannon.pos) < 6.0 * CREATURE_SCALE && !killed_cannons.contains(&i) {
                        killed_cannons.push(i);
                    }
                }
                for (i, snake) in self.snakes.iter().enumerate() {
                    if snake.alive && projectile.pos.distance(snake.pos) < snake.radius() * CREATURE_SCALE && !killed_snakes.contains(&i) {
                        killed_snakes.push(i);
                    }
                }
            }
        }

        // Apply projectile actions
        for idx in projectiles_to_kill {
            self.projectiles.projectiles[idx].alive = false;
            self.sound_events.push(SoundEvent::Attack);
        }
        for idx in projectiles_to_reflect {
            self.projectiles.projectiles[idx].reflect(player_look_dir);
            self.sound_events.push(SoundEvent::Deflect);
        }

        // Apply all collected actions (blocking bumps enemies)
        if !spider_bumps.is_empty() || !cannon_bumps.is_empty() || !snake_bumps.is_empty() {
            self.sound_events.push(SoundEvent::Block);
        }
        for (i, dir, amount) in spider_bumps {
            self.spiders[i].bump(dir, amount);
        }
        for (i, dir, amount) in cannon_bumps {
            self.cannons[i].bump(dir, amount);
        }
        for (i, dir, amount) in snake_bumps {
            self.snakes[i].bump(dir, amount);
        }

        // Kill enemies and spawn explosions
        // Also track kills for network reporting
        for i in killed_spiders {
            let pos = self.spiders[i].pos;
            let enemy_id = self.spiders[i].id as u16;
            self.spiders[i].kill();
            self.kills += 1;
            self.wave_kill_counts[0] = self.wave_kill_counts[0].saturating_add(1);
            self.explosions.spawn(pos, 7, 0, 0);
            self.sound_events.push(SoundEvent::EnemyKill);
            self.pending_enemy_kills.push((crate::net::EnemyType::Spider, enemy_id));
        }
        for i in killed_cannons {
            let pos = self.cannons[i].pos;
            let enemy_id = self.cannons[i].id as u16;
            self.cannons[i].kill();
            self.kills += 1;
            self.wave_kill_counts[1] = self.wave_kill_counts[1].saturating_add(1);
            self.explosions.spawn(pos, 8, 0, 0);
            self.sound_events.push(SoundEvent::EnemyKill);
            self.pending_enemy_kills.push((crate::net::EnemyType::Cannon, enemy_id));
        }
        for i in killed_snakes {
            let pos = self.snakes[i].pos;
            let enemy_id = self.snakes[i].id as u16;
            self.snakes[i].kill();
            self.kills += 1;
            self.wave_kill_counts[2] = self.wave_kill_counts[2].saturating_add(1);
            self.explosions.spawn(pos, 9, 0, 0);
            self.sound_events.push(SoundEvent::EnemyKill);
            self.pending_enemy_kills.push((crate::net::EnemyType::Snake, enemy_id));
        }

        // Spawn cannon projectiles
        for (pos, speed, duration) in new_projectiles {
            self.projectiles.spawn(pos, speed, duration);
        }

        // Kill player if needed
        if player_killed && self.player.alive {
            self.kill_player(player_killed_by);
        }

        // Update explosions
        self.explosions.update();

        // Check for wave progression based on kill target
        if run_enemy_ai && self.wave_targets_met() {
            self.spawn_wave_for_players(player_count);
        }
    }

    fn spawn_wave(&mut self) {
        // Default to 1 player (solo play) - multiplayer will use spawn_wave_for_players
        self.spawn_wave_for_players(1);
    }

    /// Spawn a wave scaled by player count
    /// Uses EXACT original formula scaled by player count:
    /// - Original: 160x160 = 25,600 px² visible area
    /// - Our game has a larger viewport, so we multiply by player count
    pub fn spawn_wave_for_players(&mut self, player_count: usize) {
        self.wave += 1;

        let player_scale = player_count.max(1) as u32;
        let (base_spiders, base_cannons, base_snakes) = Self::wave_base_counts(self.wave);
        let spider_count = base_spiders.saturating_mul(player_scale) as usize;
        let cannon_count = base_cannons.saturating_mul(player_scale) as usize;
        let snake_count = base_snakes.saturating_mul(player_scale) as usize;

        self.wave_kill_counts = [0; 3];
        self.wave_kill_targets = [
            spider_count as u32,
            cannon_count as u32,
            snake_count as u32,
        ];
        self.spawn_chunk_last_frame.clear();
        self.spawn_progress = 0.0;
        self.last_spawn_positions.clear();
        self.last_spawn_positions.push(self.player.pos);

        // Initial spawn so wave starts with enemies.
        let target_positions = [self.player.pos];
        self.spawn_enemies_for_movement(&target_positions, player_count, self.frame_count);

        // Create wave start event for network broadcast
        let wave_start = crate::net::WaveStart {
            wave: self.wave,
            spider_count: spider_count as u16,
            cannon_count: cannon_count as u16,
            snake_count: snake_count as u16,
            spawn_x: self.player.pos.x,
            spawn_y: self.player.pos.y,
            rng_seed: 0,
        };
        self.pending_wave_start = Some(wave_start);
        self.last_wave_start = Some(wave_start);
    }

    /// Spawn a wave using a specific RNG seed (for deterministic multiplayer)
    pub fn spawn_wave_with_seed(&mut self, rng_seed: u64, spider_count: usize, cannon_count: usize, snake_count: usize) {
        let player_pos = self.player.pos;
        self.spawn_wave_with_seed_at(rng_seed, spider_count, cannon_count, snake_count, player_pos);
    }

    pub fn spawn_wave_with_seed_at(&mut self, rng_seed: u64, spider_count: usize, cannon_count: usize, snake_count: usize, player_pos: Vec2) {
        // Set RNG to the shared seed
        self.rng = Xoshiro256PlusPlus::seed_from_u64(rng_seed);

        // Clear old enemies
        self.spiders.clear();
        self.cannons.clear();
        self.snakes.clear();

        // Spawn enemies around the player position (in infinite world)
        // Use validated spawning to avoid placing enemies in obstacles
        let spawn_distance = 400.0; // Spawn distance from player

        for id in 0..spider_count {
            self.spiders.push(Spider::new_around_validated(
                id, player_pos, spawn_distance, &self.chunks, &mut self.rng
            ));
        }

        for id in 0..cannon_count {
            self.cannons.push(Cannon::new_around_validated(
                id, player_pos, spawn_distance, &self.chunks, &mut self.rng
            ));
        }

        for id in 0..snake_count {
            let previous = if id > 0 { self.snakes.get(id - 1) } else { None };
            self.snakes.push(Snake::new_around_validated(
                id, previous, player_pos, spawn_distance, &self.chunks, &mut self.rng
            ));
        }
    }

    /// Apply a wave start event from the network
    pub fn apply_wave_start(&mut self, wave_start: &crate::net::WaveStart) {
        self.wave = wave_start.wave;
        self.wave_kill_counts = [0; 3];
        self.wave_kill_targets = [
            wave_start.spider_count as u32,
            wave_start.cannon_count as u32,
            wave_start.snake_count as u32,
        ];
        self.spawn_chunk_last_frame.clear();
        self.spawn_progress = 0.0;
        self.last_spawn_positions.clear();
    }

    /// Take pending wave start event (for network broadcast)
    pub fn take_pending_wave_start(&mut self) -> Option<crate::net::WaveStart> {
        self.pending_wave_start.take()
    }

    fn kill_player(&mut self, killed_by: Option<(u8, u16)>) {
        if self.player.alive {
            let (killed_by_type, killed_by_id) = killed_by.unwrap_or((255, 0));
            self.player.kill();
            self.deaths += 1;
            self.end_frame = self.frame_count;
            self.pending_player_deaths.push(crate::net::PlayerDeath {
                death_x: self.player.pos.x,
                death_y: self.player.pos.y,
                killed_by_type,
                killed_by_id,
                victim_hash: 0,
            });
            self.explosions.spawn(self.player.pos, 25, 0, 0);
            self.explosions.spawn(self.player.pos, 27, -3, 0);
            self.explosions.spawn(self.player.pos, 18, -8, 1);
            self.sound_events.push(SoundEvent::Death);
            self.map_open = true;
            self.map_center = self.player.pos;
            self.map_target = Some(self.map_center);
            self.map_text_input_active = false;
            self.map_active_field = 0;
            self.update_map_inputs_from_center();
            self.close_chat();
        }
    }

    fn update_gameover(&mut self, input: &Input) {
        use crate::input::{BUTTON_ATTACK, BUTTON_PHASE, BUTTON_MAP};

        if input.is_pressed(BUTTON_MAP) {
            self.toggle_map();
        }

        if self.map_open {
            self.update_map_controls(input);
            return;
        }

        if input.is_released(BUTTON_ATTACK) || input.is_released(BUTTON_PHASE) {
            self.map_open = true;
        }
    }

    pub fn toggle_map(&mut self) {
        self.map_open = !self.map_open;
        if self.map_open {
            self.map_center = self.player.pos;
            self.map_target = Some(self.map_center);
            self.map_text_input_active = false;
            self.map_active_field = 0;
            self.update_map_inputs_from_center();
            self.close_chat();
        }
    }

    pub fn toggle_player_list(&mut self) {
        self.player_list_open = !self.player_list_open;
        if !self.player_list_open {
            self.player_list_search_active = false;
        }
    }

    pub fn toggle_chat(&mut self) {
        if self.scene != Scene::Game {
            return;
        }

        self.chat_open = !self.chat_open;
        if !self.chat_open {
            self.chat_input.clear();
        }
    }

    pub fn close_chat(&mut self) {
        self.chat_open = false;
        self.chat_input.clear();
    }

    pub fn is_chat_input_active(&self) -> bool {
        self.chat_open && self.scene == Scene::Game
    }

    pub fn handle_chat_char_input(&mut self, c: char) {
        if !self.is_chat_input_active() {
            return;
        }

        if !c.is_ascii() || c.is_ascii_control() {
            return;
        }

        if self.chat_input.len() >= 80 {
            return;
        }

        self.chat_input.push(c);
    }

    pub fn handle_chat_backspace(&mut self) {
        if !self.is_chat_input_active() {
            return;
        }
        self.chat_input.pop();
    }

    pub fn take_chat_input(&mut self) -> Option<String> {
        if !self.is_chat_input_active() {
            return None;
        }

        let trimmed = self.chat_input.trim_end();
        if trimmed.is_empty() {
            self.chat_input.clear();
            return None;
        }

        let text = trimmed.to_string();
        self.chat_input.clear();
        Some(text)
    }

    pub fn can_send_chat(&self) -> bool {
        self.last_chat_send_frame == u32::MAX
            || self.frame_count.saturating_sub(self.last_chat_send_frame) >= 120
    }

    pub fn mark_chat_sent(&mut self) {
        self.last_chat_send_frame = self.frame_count;
    }

    pub fn push_chat_line(&mut self, name: String, text: String) {
        self.chat_log.push(ChatLine {
            name,
            text,
            frame: self.frame_count,
        });

        let max_lines = 8;
        if self.chat_log.len() > max_lines {
            let excess = self.chat_log.len() - max_lines;
            self.chat_log.drain(0..excess);
        }
    }

    fn prune_chat_log(&mut self) {
        let max_age_frames = 1260;
        let current = self.frame_count;
        self.chat_log.retain(|line| current.saturating_sub(line.frame) <= max_age_frames);
    }

    pub fn scroll_player_list(&mut self, delta: i32) {
        self.player_list_scroll = (self.player_list_scroll + delta).max(0);
    }

    pub fn cycle_player_list_sort(&mut self) {
        self.player_list_sort = (self.player_list_sort + 1) % 5;
    }

    pub fn toggle_player_list_sort_order(&mut self) {
        self.player_list_sort_asc = !self.player_list_sort_asc;
    }

    pub fn activate_player_list_search(&mut self) {
        self.player_list_search_active = true;
    }

    pub fn clear_player_list_search(&mut self) {
        self.player_list_search.clear();
        self.player_list_search_active = false;
        self.player_list_scroll = 0;
    }

    pub fn handle_player_list_char_input(&mut self, c: char) {
        if self.player_list_search.len() >= 16 {
            return;
        }
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            self.player_list_search.push(c.to_ascii_lowercase());
        }
    }

    pub fn handle_player_list_backspace(&mut self) {
        self.player_list_search.pop();
    }

    fn update_map_controls(&mut self, input: &Input) {
        use crate::input::{BUTTON_ATTACK, BUTTON_PHASE};
        use crate::world::CHUNK_SIZE;

        if self.map_text_input_active {
            return;
        }

        if input.is_pressed(BUTTON_ATTACK) {
            self.map_zoom = (self.map_zoom * MAP_ZOOM_STEP).min(MAP_ZOOM_MAX);
        }
        if input.is_pressed(BUTTON_PHASE) {
            self.map_zoom = (self.map_zoom / MAP_ZOOM_STEP).max(MAP_ZOOM_MIN);
        }

        let map_size = MAP_OVERLAY_SIZE;
        let base_world_span = CHUNK_SIZE as f32 * 8.0;
        let pixels_per_world = map_size / base_world_span * self.map_zoom;
        let world_per_pixel = if pixels_per_world > 0.0 { 1.0 / pixels_per_world } else { 1.0 };
        let pan_speed = world_per_pixel * 10.0;

        if input.axis.x != 0.0 || input.axis.y != 0.0 {
            self.map_center += input.axis * pan_speed;
        }

        if !self.map_text_input_active {
            self.map_target = Some(self.map_center);
            self.update_map_inputs_from_center();
        }
    }

    pub fn pan_map_by_screen_delta(&mut self, dx: f32, dy: f32) {
        use crate::world::CHUNK_SIZE;
        let base_world_span = CHUNK_SIZE as f32 * 8.0;
        let pixels_per_world = MAP_OVERLAY_SIZE / base_world_span * self.map_zoom;
        if pixels_per_world <= 0.0 {
            return;
        }
        self.map_center.x += dx / pixels_per_world;
        self.map_center.y += dy / pixels_per_world;
        self.map_target = Some(self.map_center);
        self.update_map_inputs_from_center();
    }

    pub fn zoom_map_in(&mut self) {
        self.map_zoom = (self.map_zoom * MAP_ZOOM_STEP).min(MAP_ZOOM_MAX);
        self.map_target = Some(self.map_center);
        self.update_map_inputs_from_center();
    }

    pub fn zoom_map_out(&mut self) {
        self.map_zoom = (self.map_zoom / MAP_ZOOM_STEP).max(MAP_ZOOM_MIN);
        self.map_target = Some(self.map_center);
        self.update_map_inputs_from_center();
    }

    pub fn handle_map_click(&mut self, screen_x: f64, screen_y: f64) {
        use crate::world::CHUNK_SIZE;

        if !self.map_open {
            return;
        }

        let map_left = MAP_OVERLAY_PADDING as f64;
        let map_top = MAP_OVERLAY_PADDING as f64;
        let map_size = MAP_OVERLAY_SIZE as f64;
        if screen_x < map_left || screen_x > map_left + map_size || screen_y < map_top || screen_y > map_top + map_size {
            return;
        }

        let base_world_span = CHUNK_SIZE as f64 * 8.0;
        let pixels_per_world = map_size / base_world_span * self.map_zoom as f64;
        let center_x = map_left + map_size / 2.0;
        let center_y = map_top + map_size / 2.0;
        let world_x = self.map_center.x as f64 + (screen_x - center_x) / pixels_per_world;
        let world_y = self.map_center.y as f64 + (screen_y - center_y) / pixels_per_world;

        self.map_center = Vec2::new(world_x as f32, world_y as f32);
        self.map_target = Some(self.map_center);
        self.update_map_inputs_from_center();
    }

    pub fn activate_map_input(&mut self, field: u8) {
        if !self.map_open {
            return;
        }
        self.map_text_input_active = true;
        self.map_active_field = field.min(1);
    }

    pub fn is_map_input_active(&self) -> bool {
        self.map_open && self.map_text_input_active
    }

    pub fn handle_map_char_input(&mut self, c: char) {
        if !self.map_open {
            return;
        }

        if !(c.is_ascii_digit() || c == '-' || c == '.') {
            return;
        }

        let input = if self.map_active_field == 0 {
            &mut self.map_input_x
        } else {
            &mut self.map_input_y
        };

        if input.len() >= 12 {
            return;
        }

        input.push(c);
        self.try_update_map_center_from_inputs();
    }

    pub fn handle_map_backspace(&mut self) {
        if !self.map_open {
            return;
        }

        let input = if self.map_active_field == 0 {
            &mut self.map_input_x
        } else {
            &mut self.map_input_y
        };

        input.pop();
        self.try_update_map_center_from_inputs();
    }

    pub fn set_map_target_from_inputs(&mut self) -> bool {
        if !self.map_open {
            return false;
        }

        let x: f32 = match self.map_input_x.trim().parse() {
            Ok(val) => val,
            Err(_) => return false,
        };
        let y: f32 = match self.map_input_y.trim().parse() {
            Ok(val) => val,
            Err(_) => return false,
        };

        self.map_center = Vec2::new(x, y);
        self.map_target = Some(self.map_center);
        self.update_map_inputs_from_center();
        true
    }

    fn update_map_inputs_from_center(&mut self) {
        self.map_input_x = format!("{:.0}", self.map_center.x);
        self.map_input_y = format!("{:.0}", self.map_center.y);
    }

    fn try_update_map_center_from_inputs(&mut self) {
        if let (Ok(x), Ok(y)) = (
            self.map_input_x.trim().parse::<f32>(),
            self.map_input_y.trim().parse::<f32>(),
        ) {
            self.map_center = Vec2::new(x, y);
            self.map_target = Some(self.map_center);
        }
    }

    pub fn confirm_map_teleport_from_inputs(&mut self) {
        if self.set_map_target_from_inputs() {
            self.confirm_map_teleport();
        } else {
            self.confirm_map_teleport();
        }
    }

    pub fn confirm_map_teleport(&mut self) {
        if !self.map_open {
            return;
        }

        if let Some(target) = self.map_target {
            self.chunks.update(target);

            if !self.player.alive {
                self.player = Player::new_at_position(target);
                self.scene = Scene::Game;
            } else {
                self.player.pos = target;
            }

            self.map_open = false;
            self.map_center = target;
            self.map_target = None;
        }
    }

    pub fn apply_paid_obstacle(&mut self, obstacle: crate::net::PaidObstacle) -> bool {
        if self.paid_obstacle_hashes.contains(&obstacle.proof_hash) {
            return false;
        }

        let pos = Vec2::new(obstacle.x, obstacle.y);
        self.chunks.update(pos);

        let obstacle_data = crate::world::Obstacle {
            pos,
            radius: obstacle.radius,
            variant: obstacle.variant,
        };
        self.chunks.add_dynamic_obstacle(obstacle_data.clone());
        self.paid_obstacles.push(obstacle);
        self.paid_obstacle_hashes.insert(obstacle.proof_hash);
        self.paid_obstacle_map.insert(obstacle.proof_hash, obstacle_data);
        true
    }

    pub fn remove_paid_obstacle(&mut self, proof_hash: [u8; 32]) {
        if let Some(obstacle) = self.paid_obstacle_map.remove(&proof_hash) {
            self.chunks.remove_dynamic_obstacle(&obstacle);
        }
        self.paid_obstacles.retain(|obs| obs.proof_hash != proof_hash);
        self.paid_obstacle_hashes.remove(&proof_hash);
    }

    pub fn store_paid_obstacle_candidate(&mut self, obstacle: crate::net::PaidObstacle) {
        self.pending_paid_obstacle_candidates
            .insert(obstacle.proof_hash, obstacle);
    }

    pub fn take_paid_obstacle_candidate(&mut self, proof_hash: [u8; 32]) -> Option<crate::net::PaidObstacle> {
        self.pending_paid_obstacle_candidates.remove(&proof_hash)
    }

    pub fn pending_paid_obstacle_hashes(&self) -> Vec<[u8; 32]> {
        self.pending_paid_obstacle_candidates.keys().copied().collect()
    }

    pub fn place_paid_obstacle(&mut self, obstacle: crate::net::PaidObstacle) -> bool {
        if self.apply_paid_obstacle(obstacle) {
            self.pending_paid_obstacles.push(obstacle);
            return true;
        }
        false
    }

    pub fn take_pending_paid_obstacles(&mut self) -> Vec<crate::net::PaidObstacle> {
        std::mem::take(&mut self.pending_paid_obstacles)
    }

    pub fn take_pending_cannon_shots(&mut self) -> Vec<crate::net::CannonShot> {
        std::mem::take(&mut self.pending_cannon_shots)
    }

    /// Respawn player in multiplayer - keeps enemies and wave, just revives player
    pub fn respawn_in_multiplayer(&mut self) {
        self.player = Player::new_at_position(Vec2::ZERO);
        self.start_frame = self.frame_count;
        self.scene = Scene::Game;
        // Request enemy sync from host
        self.pending_respawn = true;
    }

    /// Check if we need enemy sync after respawn
    pub fn needs_respawn_sync(&self) -> bool {
        self.pending_respawn
    }

    /// Clear respawn sync request (after receiving sync)
    pub fn clear_respawn_sync(&mut self) {
        self.pending_respawn = false;
    }

    fn start_game(&mut self) {
        self.rng = Xoshiro256PlusPlus::seed_from_u64(self.frame_count as u64);
        self.start_frame = self.frame_count;
        self.wave = 0;
        self.kills = 0;
        self.deaths = 0;
        self.player = Player::new_at_position(Vec2::ZERO);
        self.camera = Camera::new(self.width, self.height, CAMERA_ZOOM);
        self.chunks = ChunkManager::new(self.world_seed);
        self.explored_chunks.clear();
        self.map_open = false;
        self.map_zoom = 1.0;
        self.map_center = Vec2::ZERO;
        self.map_target = None;
        self.map_text_input_active = false;
        self.map_active_field = 0;
        self.map_input_x.clear();
        self.map_input_y.clear();
        self.paid_obstacles.clear();
        self.paid_obstacle_hashes.clear();
        self.paid_obstacle_map.clear();
        self.pending_paid_obstacle_candidates.clear();
        self.pending_paid_obstacles.clear();
        self.pending_cannon_shots.clear();
        self.queued_join_room = false;
        self.input_history.clear();
        self.remote_simulations.clear();
        self.remote_predictions.clear();
        self.wave_kill_counts = [0; 3];
        self.wave_kill_targets = [0; 3];
        self.spawn_chunk_last_frame.clear();
        self.spawn_progress = 0.0;
        self.last_spawn_positions.clear();
        self.player_list_open = false;
        self.player_list_scroll = 0;
        self.player_list_sort = 0;
        self.player_list_sort_asc = false;
        self.player_list_search.clear();
        self.player_list_search_active = false;
        self.chat_open = false;
        self.chat_input.clear();
        self.chat_log.clear();
        self.last_chat_send_frame = u32::MAX;
        self.spiders.clear();
        self.cannons.clear();
        self.snakes.clear();
        self.projectiles.clear();
        self.explosions.clear();
        self.pending_player_deaths.clear();
        self.scene = Scene::Game;
    }

    /// Start game for multiplayer - preserves player name and room code
    pub fn start_game_with_network(&mut self) {
        if !self.room_code_input.is_empty() {
            self.world_seed = seed_from_room_code(&self.room_code_input);
        }
        self.rng = Xoshiro256PlusPlus::seed_from_u64(self.frame_count as u64);
        self.start_frame = self.frame_count;
        self.wave = 0;
        self.kills = 0;
        self.deaths = 0;
        self.player = Player::new_at_position(Vec2::ZERO);
        self.camera = Camera::new(self.width, self.height, CAMERA_ZOOM);
        self.chunks = ChunkManager::new(self.world_seed);
        self.explored_chunks.clear();
        self.map_open = false;
        self.map_zoom = 1.0;
        self.map_center = Vec2::ZERO;
        self.map_target = None;
        self.map_text_input_active = false;
        self.map_active_field = 0;
        self.map_input_x.clear();
        self.map_input_y.clear();
        self.paid_obstacles.clear();
        self.paid_obstacle_hashes.clear();
        self.paid_obstacle_map.clear();
        self.pending_paid_obstacle_candidates.clear();
        self.pending_paid_obstacles.clear();
        self.pending_cannon_shots.clear();
        self.queued_join_room = false;
        self.input_history.clear();
        self.remote_simulations.clear();
        self.remote_predictions.clear();
        self.wave_kill_counts = [0; 3];
        self.wave_kill_targets = [0; 3];
        self.spawn_chunk_last_frame.clear();
        self.spawn_progress = 0.0;
        self.last_spawn_positions.clear();
        self.player_list_open = false;
        self.player_list_scroll = 0;
        self.player_list_sort = 0;
        self.player_list_sort_asc = false;
        self.player_list_search.clear();
        self.player_list_search_active = false;
        self.chat_open = false;
        self.chat_input.clear();
        self.chat_log.clear();
        self.last_chat_send_frame = u32::MAX;
        self.spiders.clear();
        self.cannons.clear();
        self.snakes.clear();
        self.projectiles.clear();
        self.explosions.clear();
        self.pending_player_deaths.clear();
        self.scene = Scene::Game;
    }

    /// Debug helper - start a fresh single-player game
    pub fn debug_start_game(&mut self) {
        self.start_game();
    }

    /// Create enemy sync data for broadcasting (host only)
    pub fn create_enemy_sync(&self) -> crate::net::EnemySync {
        use crate::net::EnemyState;

        let mut enemies = Vec::new();

        // Add spiders (alive or dead so clients can clear dead ones)
        for spider in &self.spiders {
            enemies.push(EnemyState::new_spider(
                spider.id,
                spider.alive,
                spider.pos,
                spider.dir,
            ));
        }

        // Add cannons (alive or dead so clients can clear dead ones)
        for cannon in &self.cannons {
            enemies.push(EnemyState::new_cannon(
                cannon.id,
                cannon.alive,
                cannon.pos,
                cannon.look_dir,
            ));
        }

        // Add snakes (alive or dead so clients can clear dead ones)
        for snake in &self.snakes {
            enemies.push(EnemyState::new_snake(
                snake.id,
                snake.alive,
                snake.pos,
                snake.dir,
                snake.size,
            ));
        }

        crate::net::EnemySync {
            wave: self.wave,
            enemies,
        }
    }

    fn wave_base_counts(wave: u32) -> (u32, u32, u32) {
        let is_boss = wave % 10 == 0;
        if is_boss {
            let snake_count = (wave as u32).min(50);
            let base = wave.saturating_sub(10) / 4;
            let cannon_count = (base / 3).min(20);
            let spider_count = base.saturating_sub(cannon_count).min(100);
            (spider_count, cannon_count, snake_count)
        } else {
            let cannon_count = (wave / 3).min(20);
            let spider_count = wave.saturating_sub(cannon_count).min(100);
            (spider_count, cannon_count, 0)
        }
    }

    fn wave_targets_met(&self) -> bool {
        for (count, target) in self.wave_kill_counts.iter().zip(self.wave_kill_targets.iter()) {
            if *target > 0 && *count < *target {
                return false;
            }
        }
        true
    }

    fn spawn_enemies_for_chunks(&mut self, chunks: &[(i32, i32)], player_count: usize, avoid_positions: &[Vec2], current_frame: u32) {
        use crate::world::CHUNK_SIZE;
        if self.wave == 0 {
            return;
        }

        let player_scale = player_count.max(1) as f32;
        let (base_spiders, base_cannons, base_snakes) = Self::wave_base_counts(self.wave);
        let reference_area = self.width as f32 * self.height as f32;
        let chunk_area = (CHUNK_SIZE as f32) * (CHUNK_SIZE as f32);

        for &(cx, cy) in chunks {
            if let Some(last_frame) = self.spawn_chunk_last_frame.get(&(cx, cy)) {
                if current_frame.saturating_sub(*last_frame) < SPAWN_CHUNK_COOLDOWN_FRAMES {
                    continue;
                }
            }

            let chunk = match self.chunks.chunks.get(&(cx, cy)) {
                Some(c) => c,
                None => continue,
            };

            let seed = chunk.enemy_spawn_seed ^ (self.wave as u64).wrapping_mul(0x9E3779B97F4A7C15);
            let mut rng = Xoshiro256PlusPlus::seed_from_u64(seed);

            let spider_f = (base_spiders as f32 * player_scale) * (chunk_area / reference_area);
            let cannon_f = (base_cannons as f32 * player_scale) * (chunk_area / reference_area);
            let snake_f = (base_snakes as f32 * player_scale) * (chunk_area / reference_area);

            let spider_count = Self::sample_spawn_count(spider_f, &mut rng);
            let cannon_count = Self::sample_spawn_count(cannon_f, &mut rng);
            let snake_count = Self::sample_spawn_count(snake_f, &mut rng);

            let origin = Vec2::new((cx * CHUNK_SIZE) as f32, (cy * CHUNK_SIZE) as f32);
            let max_offset = CHUNK_SIZE as f32;

            for _ in 0..spider_count {
                if let Some(pos) = Self::random_point_in_chunk(&mut rng, origin, max_offset, &self.chunks, 8.0, avoid_positions, SPAWN_SAFE_DISTANCE) {
                    let id = self.spiders.len();
                    self.spiders.push(Spider::new_at_position(id, pos, &mut rng));
                }
            }

            for _ in 0..cannon_count {
                if let Some(pos) = Self::random_point_in_chunk(&mut rng, origin, max_offset, &self.chunks, 10.0, avoid_positions, SPAWN_SAFE_DISTANCE) {
                    let id = self.cannons.len();
                    self.cannons.push(Cannon::new_at_position(id, pos, &mut rng));
                }
            }

            if snake_count > 0 {
                let mut previous: Option<Snake> = None;
                for _ in 0..snake_count {
                    let pos = if let Some(spawn) = Self::random_point_in_chunk(&mut rng, origin, max_offset, &self.chunks, 12.0, avoid_positions, SPAWN_SAFE_DISTANCE) {
                        spawn
                    } else {
                        origin + Vec2::new(max_offset * 0.5, max_offset * 0.5)
                    };
                    let id = self.snakes.len();
                    let snake = Snake::new_at_position(id, previous.as_ref(), pos);
                    previous = Some(snake.clone());
                    self.snakes.push(snake);
                }
            }

            self.spawn_chunk_last_frame.insert((cx, cy), current_frame);
        }
    }

    fn spawn_enemies_for_movement(&mut self, target_positions: &[Vec2], player_count: usize, current_frame: u32) {
        if self.wave == 0 {
            return;
        }
        let (base_spiders, base_cannons, base_snakes) = Self::wave_base_counts(self.wave);
        let weights = [
            base_spiders.max(1),
            base_cannons,
            base_snakes,
        ];
        let total: u32 = weights.iter().sum();
        if total == 0 {
            return;
        }

        let spawn_count = player_count.max(1);
        for _ in 0..spawn_count {
            let roll = self.rng.gen_range(0..total);
            let enemy_type = if roll < weights[0] {
                crate::net::EnemyType::Spider
            } else if roll < weights[0] + weights[1] {
                crate::net::EnemyType::Cannon
            } else {
                crate::net::EnemyType::Snake
            };
            self.spawn_enemy_near_players(enemy_type, target_positions, current_frame);
        }
    }

    fn spawn_enemy_near_players(&mut self, enemy_type: crate::net::EnemyType, target_positions: &[Vec2], current_frame: u32) -> bool {
        use crate::world::CHUNK_SIZE;

        if target_positions.is_empty() {
            return false;
        }

        let mut candidates: Vec<(i32, i32)> = Vec::new();
        for pos in target_positions {
            let cx = (pos.x / CHUNK_SIZE as f32).floor() as i32;
            let cy = (pos.y / CHUNK_SIZE as f32).floor() as i32;
            for dx in -1..=1 {
                for dy in -1..=1 {
                    candidates.push((cx + dx, cy + dy));
                }
            }
        }
        candidates.sort();
        candidates.dedup();

        if candidates.is_empty() {
            return false;
        }

        let max_attempts = candidates.len().min(12);
        for _ in 0..max_attempts {
            let idx = self.rng.gen_range(0..candidates.len());
            let chunk = candidates[idx];
            if !self.chunks.chunks.contains_key(&chunk) {
                continue;
            }
            if let Some(last_frame) = self.spawn_chunk_last_frame.get(&chunk) {
                if current_frame.saturating_sub(*last_frame) < SPAWN_CHUNK_COOLDOWN_FRAMES {
                    continue;
                }
            }
            let origin = Vec2::new((chunk.0 * CHUNK_SIZE) as f32, (chunk.1 * CHUNK_SIZE) as f32);
            let max_offset = CHUNK_SIZE as f32;
            let radius = match enemy_type {
                crate::net::EnemyType::Spider => 8.0,
                crate::net::EnemyType::Cannon => 10.0,
                crate::net::EnemyType::Snake => 12.0,
            };

            if let Some(pos) = Self::random_point_in_chunk(
                &mut self.rng,
                origin,
                max_offset,
                &self.chunks,
                radius,
                target_positions,
                SPAWN_SAFE_DISTANCE,
            ) {
                match enemy_type {
                    crate::net::EnemyType::Spider => {
                        let id = self.spiders.len();
                        self.spiders.push(Spider::new_at_position(id, pos, &mut self.rng));
                    }
                    crate::net::EnemyType::Cannon => {
                        let id = self.cannons.len();
                        self.cannons.push(Cannon::new_at_position(id, pos, &mut self.rng));
                    }
                    crate::net::EnemyType::Snake => {
                        let id = self.snakes.len();
                        let snake = Snake::new_at_position(id, None, pos);
                        self.snakes.push(snake);
                    }
                }
                self.spawn_chunk_last_frame.insert(chunk, current_frame);
                return true;
            }
        }

        false
    }

    fn random_point_in_chunk(
        rng: &mut Xoshiro256PlusPlus,
        origin: Vec2,
        size: f32,
        chunks: &ChunkManager,
        radius: f32,
        avoid_positions: &[Vec2],
        min_distance: f32,
    ) -> Option<Vec2> {
        for _ in 0..10 {
            let x = origin.x + rng.gen::<f32>() * size;
            let y = origin.y + rng.gen::<f32>() * size;
            let pos = Vec2::new(x, y);
            if chunks.collides_with_obstacle(pos, radius) {
                continue;
            }
            let mut too_close = false;
            for avoid in avoid_positions {
                if pos.distance(*avoid) < min_distance {
                    too_close = true;
                    break;
                }
            }
            if !too_close {
                return Some(pos);
            }
        }
        None
    }

    fn sample_spawn_count(value: f32, rng: &mut Xoshiro256PlusPlus) -> usize {
        if value <= 0.0 {
            return 0;
        }
        let base = value.floor();
        let frac = value - base;
        let extra = if rng.gen::<f32>() < frac { 1.0 } else { 0.0 };
        (base + extra) as usize
    }

    /// Apply enemy sync from host (client only)
    pub fn apply_enemy_sync(&mut self, sync: &crate::net::EnemySync) {
        use crate::net::EnemyType;

        // Update wave if it changed
        if sync.wave != self.wave {
            self.wave = sync.wave;
            // Clear local enemies to rebuild from sync
            self.spiders.clear();
            self.cannons.clear();
            self.snakes.clear();
        }

        // Process each enemy in the sync
        for enemy_state in &sync.enemies {
            let enemy_type = match enemy_state.get_type() {
                Some(t) => t,
                None => continue,
            };

            match enemy_type {
                EnemyType::Spider => {
                    let id = enemy_state.id as usize;
                    // Find existing spider or create placeholder
                    while self.spiders.len() <= id {
                        // Create placeholder spider that will be updated
                        self.spiders.push(Spider::new_around(
                            self.spiders.len(),
                            enemy_state.pos(),
                            0.0,
                            &mut self.rng,
                        ));
                    }
                    // Update spider state
                    let spider = &mut self.spiders[id];
                    spider.alive = enemy_state.is_alive();
                    spider.pos = enemy_state.pos();
                    spider.dir = enemy_state.dir();
                }
                EnemyType::Cannon => {
                    let id = enemy_state.id as usize;
                    while self.cannons.len() <= id {
                        self.cannons.push(Cannon::new_around(
                            self.cannons.len(),
                            enemy_state.pos(),
                            0.0,
                            &mut self.rng,
                        ));
                    }
                    let cannon = &mut self.cannons[id];
                    let prev_pos = cannon.pos;
                    cannon.alive = enemy_state.is_alive();
                    cannon.pos = enemy_state.pos();
                    cannon.look_dir = enemy_state.dir();
                    // Update dir based on movement so wheel spacing matches host view.
                    let delta = cannon.pos - prev_pos;
                    if delta.length() > 0.0 {
                        cannon.dir = delta.normalize();
                    }
                }
                EnemyType::Snake => {
                    let id = enemy_state.id as usize;
                    while self.snakes.len() <= id {
                        let previous = if self.snakes.is_empty() {
                            None
                        } else {
                            self.snakes.last()
                        };
                        self.snakes.push(Snake::new_around(
                            self.snakes.len(),
                            previous,
                            enemy_state.pos(),
                            0.0,
                            &mut self.rng,
                        ));
                    }
                    let snake = &mut self.snakes[id];
                    snake.alive = enemy_state.is_alive();
                    snake.pos = enemy_state.pos();
                    snake.dir = enemy_state.dir();
                    snake.size = enemy_state.snake_size();
                }
            }
        }
    }

    /// Kill a specific enemy by type and ID (used for network damage sync from other players)
    /// Does NOT increment local kills - this is for remote player kills only
    pub fn kill_enemy(&mut self, enemy_type: crate::net::EnemyType, enemy_id: u16) {
        use crate::net::EnemyType;

        let id = enemy_id as usize;
        match enemy_type {
            EnemyType::Spider => {
                if id < self.spiders.len() && self.spiders[id].alive {
                    let pos = self.spiders[id].pos;
                    self.spiders[id].kill();
                    self.wave_kill_counts[0] = self.wave_kill_counts[0].saturating_add(1);
                    // Don't increment self.kills - this is a remote player's kill
                    self.explosions.spawn(pos, 7, 0, 0);
                    self.sound_events.push(SoundEvent::EnemyKill);
                }
            }
            EnemyType::Cannon => {
                if id < self.cannons.len() && self.cannons[id].alive {
                    let pos = self.cannons[id].pos;
                    self.cannons[id].kill();
                    self.wave_kill_counts[1] = self.wave_kill_counts[1].saturating_add(1);
                    // Don't increment self.kills - this is a remote player's kill
                    self.explosions.spawn(pos, 8, 0, 0);
                    self.sound_events.push(SoundEvent::EnemyKill);
                }
            }
            EnemyType::Snake => {
                if id < self.snakes.len() && self.snakes[id].alive {
                    let pos = self.snakes[id].pos;
                    self.snakes[id].kill();
                    self.wave_kill_counts[2] = self.wave_kill_counts[2].saturating_add(1);
                    // Don't increment self.kills - this is a remote player's kill
                    self.explosions.spawn(pos, 9, 0, 0);
                    self.sound_events.push(SoundEvent::EnemyKill);
                }
            }
        }
    }

    /// Get all remote player positions (for enemy targeting)
    pub fn get_all_player_positions(&self, remote_players: &std::collections::HashMap<String, crate::net::RemotePlayer>) -> Vec<Vec2> {
        let mut positions = vec![self.player.pos];
        for remote in remote_players.values() {
            if remote.alive {
                positions.push(remote.pos);
            }
        }
        positions
    }

    /// Find the closest player position to a given point
    pub fn find_closest_player(&self, pos: Vec2, remote_players: &std::collections::HashMap<String, crate::net::RemotePlayer>) -> Vec2 {
        let mut closest = self.player.pos;
        let mut closest_dist = pos.distance(self.player.pos);

        for remote in remote_players.values() {
            if remote.alive {
                let dist = pos.distance(remote.pos);
                if dist < closest_dist {
                    closest = remote.pos;
                    closest_dist = dist;
                }
            }
        }

        closest
    }

    /// Apply debug commands from JS/console.
    pub fn apply_debug_command(&mut self, command: &str) -> Result<String, String> {
        let mut parts = command.split_whitespace();
        let cmd = parts.next().ok_or_else(|| "empty command".to_string())?.to_lowercase();

        match cmd.as_str() {
            "help" => Ok("commands: help, start_game, teleport x y, set_wave n, set_kills n, set_deaths n, spawn_wave [players], spawn_counts spiders cannons snakes, clear_enemies, kill_player, respawn, drop_obstacle x y radius variant [proof_hash]".to_string()),
            "start_game" => {
                self.debug_start_game();
                Ok("started game".to_string())
            }
            "teleport" | "set_pos" => {
                let x: f32 = parts.next().ok_or_else(|| "missing x".to_string())?.parse().map_err(|_| "invalid x".to_string())?;
                let y: f32 = parts.next().ok_or_else(|| "missing y".to_string())?.parse().map_err(|_| "invalid y".to_string())?;
                self.player.pos = Vec2::new(x, y);
                Ok(format!("teleported to ({:.1}, {:.1})", x, y))
            }
            "set_wave" => {
                let wave: u32 = parts.next().ok_or_else(|| "missing wave".to_string())?.parse().map_err(|_| "invalid wave".to_string())?;
                self.wave = wave;
                Ok(format!("wave set to {}", wave))
            }
            "set_kills" => {
                let kills: u32 = parts.next().ok_or_else(|| "missing kills".to_string())?.parse().map_err(|_| "invalid kills".to_string())?;
                self.kills = kills;
                Ok(format!("kills set to {}", kills))
            }
            "set_deaths" => {
                let deaths: u32 = parts.next().ok_or_else(|| "missing deaths".to_string())?.parse().map_err(|_| "invalid deaths".to_string())?;
                self.deaths = deaths;
                Ok(format!("deaths set to {}", deaths))
            }
            "spawn_wave" => {
                let player_count: usize = match parts.next() {
                    Some(val) => val.parse().map_err(|_| "invalid player_count".to_string())?,
                    None => 1,
                };
                self.spawn_wave_for_players(player_count);
                Ok(format!("spawned wave {} (players={})", self.wave, player_count))
            }
            "spawn_counts" => {
                let spiders: usize = parts.next().ok_or_else(|| "missing spider count".to_string())?.parse().map_err(|_| "invalid spider count".to_string())?;
                let cannons: usize = parts.next().ok_or_else(|| "missing cannon count".to_string())?.parse().map_err(|_| "invalid cannon count".to_string())?;
                let snakes: usize = parts.next().ok_or_else(|| "missing snake count".to_string())?.parse().map_err(|_| "invalid snake count".to_string())?;
                let seed = (self.frame_count as u64).wrapping_mul(1664525).wrapping_add(1013904223);
                self.spawn_wave_with_seed(seed, spiders, cannons, snakes);
                Ok(format!("spawned enemies (spiders={}, cannons={}, snakes={})", spiders, cannons, snakes))
            }
            "clear_enemies" => {
                self.spiders.clear();
                self.cannons.clear();
                self.snakes.clear();
                self.projectiles.clear();
                Ok("cleared enemies and projectiles".to_string())
            }
            "kill_player" => {
                self.kill_player(None);
                Ok("player killed".to_string())
            }
            "respawn" => {
                self.respawn_in_multiplayer();
                Ok("player respawned".to_string())
            }
            "drop_obstacle" => {
                let x: f32 = parts.next().ok_or_else(|| "missing x".to_string())?.parse().map_err(|_| "invalid x".to_string())?;
                let y: f32 = parts.next().ok_or_else(|| "missing y".to_string())?.parse().map_err(|_| "invalid y".to_string())?;
                let radius: f32 = parts.next().ok_or_else(|| "missing radius".to_string())?.parse().map_err(|_| "invalid radius".to_string())?;
                let variant: u8 = parts.next().ok_or_else(|| "missing variant".to_string())?.parse().map_err(|_| "invalid variant".to_string())?;
                let proof_hash = if let Some(hex) = parts.next() {
                    parse_proof_hash(hex)?
                } else {
                    let mut hash = [0u8; 32];
                    hash[0..4].copy_from_slice(&self.frame_count.to_le_bytes());
                    hash
                };
                let obstacle = crate::net::PaidObstacle {
                    x,
                    y,
                    radius,
                    variant,
                    proof_hash,
                };
                if self.place_paid_obstacle(obstacle) {
                    Ok(format!("paid obstacle placed at ({:.1}, {:.1})", x, y))
                } else {
                    Err("failed to place paid obstacle".to_string())
                }
            }
            _ => Err("unknown command (try: help)".to_string()),
        }
    }

    /// Take pending enemy kills (for reporting to network)
    pub fn take_pending_kills(&mut self) -> Vec<(crate::net::EnemyType, u16)> {
        std::mem::take(&mut self.pending_enemy_kills)
    }

    /// Take pending player deaths (for reporting to network)
    pub fn take_pending_deaths(&mut self) -> Vec<crate::net::PlayerDeath> {
        std::mem::take(&mut self.pending_player_deaths)
    }

    /// Static helper to find closest target from a list of positions
    fn find_closest_target(pos: Vec2, targets: &[Vec2]) -> Vec2 {
        if targets.is_empty() {
            return Vec2::ZERO;
        }

        let mut closest = targets[0];
        let mut closest_dist = pos.distance(targets[0]);

        for &target in targets.iter().skip(1) {
            let dist = pos.distance(target);
            if dist < closest_dist {
                closest = target;
                closest_dist = dist;
            }
        }

        closest
    }

}

impl RemoteSimulation {
    fn new_from_state(state: &PlayerState, frame: u32) -> Self {
        let mut player = Player::new_at_position(state.pos());
        player.apply_net_state(state);
        let mut snapshots = std::collections::VecDeque::new();
        snapshots.push_back((frame, player.clone(), 0));
        Self {
            player,
            last_sim_frame: frame,
            last_authoritative_frame: frame,
            last_input: 0,
            pending_inputs: Vec::new(),
            snapshots,
            rollback_from: None,
        }
    }

    fn new_placeholder(frame: u32) -> Self {
        let player = Player::new_at_position(Vec2::ZERO);
        let mut snapshots = std::collections::VecDeque::new();
        snapshots.push_back((frame, player.clone(), 0));
        Self {
            player,
            last_sim_frame: frame,
            last_authoritative_frame: frame,
            last_input: 0,
            pending_inputs: Vec::new(),
            snapshots,
            rollback_from: None,
        }
    }

    fn apply_authoritative_state(&mut self, state: &PlayerState, frame: u32) {
        self.player.apply_net_state(state);
        self.last_sim_frame = frame;
        self.last_authoritative_frame = frame;
        self.snapshots.clear();
        self.snapshots.push_back((frame, self.player.clone(), self.last_input));
        self.rollback_from = None;
    }

    fn queue_input(&mut self, frame: u32, input: u8) {
        if self.pending_inputs.iter().any(|entry| entry.frame == frame) {
            return;
        }
        self.pending_inputs.push(crate::net::InputFrame { frame, input });
        self.pending_inputs.sort_by_key(|entry| entry.frame);

        if frame <= self.last_sim_frame {
            let rollback_frame = self.rollback_from.unwrap_or(frame).min(frame);
            self.rollback_from = Some(rollback_frame);
        }
    }

    fn simulate_to(&mut self, target_frame: u32, chunks: &ChunkManager) {
        if target_frame <= self.last_sim_frame {
            return;
        }

        if let Some(rollback_frame) = self.rollback_from.take() {
            if let Some((frame, player, last_input)) = self
                .snapshots
                .iter()
                .rev()
                .find(|(frame, _, _)| *frame <= rollback_frame)
                .cloned()
            {
                self.player = player;
                self.last_sim_frame = frame;
                self.last_input = last_input;
            }
        }

        let mut frame = self.last_sim_frame + 1;
        while frame <= target_frame {
            if let Some(index) = self.pending_inputs.iter().position(|entry| entry.frame == frame) {
                let entry = self.pending_inputs.remove(index);
                self.last_input = entry.input;
            }

            let prev_input = self
                .snapshots
                .back()
                .map(|(_, _, input)| *input)
                .unwrap_or(self.last_input);
            let input = Input::from_raw(self.last_input, prev_input);
            self.player.update_infinite(&input, chunks);

            self.snapshots.push_back((frame, self.player.clone(), self.last_input));
            while self.snapshots.len() > (ROLLBACK_WINDOW_FRAMES as usize + 1) {
                self.snapshots.pop_front();
            }
            frame += 1;
        }

        self.last_sim_frame = target_frame;
        self.pending_inputs.retain(|entry| entry.frame > target_frame.saturating_sub(ROLLBACK_WINDOW_FRAMES));
    }

    fn predicted_state(&self) -> PlayerState {
        PlayerState::new(
            self.player.pos,
            self.player.look_dir,
            self.player.move_dir,
            self.player.alive,
            self.player.is_attacking(),
            self.player.blocking,
            self.player.is_phasing(),
        )
    }
}

fn seed_from_room_code(room_code: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for b in room_code.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn parse_proof_hash(hex: &str) -> Result<[u8; 32], String> {
    let hex = hex.trim();
    if hex.len() != 64 {
        return Err("proof_hash must be 64 hex chars".to_string());
    }

    let mut bytes = [0u8; 32];
    for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
        let hi = (chunk[0] as char).to_digit(16).ok_or_else(|| "invalid hex".to_string())?;
        let lo = (chunk[1] as char).to_digit(16).ok_or_else(|| "invalid hex".to_string())?;
        bytes[i] = ((hi << 4) | lo) as u8;
    }
    Ok(bytes)
}

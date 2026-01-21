use matchbox_socket::{PeerState, WebRtcSocket};
use crate::net::{NetMessage, PlayerState, RemotePlayer, EnemySync, EnemyDamage, WaveStart, EnemyKill, PlayerDeath, PaidObstacle, PaidObstacleSync, PaidObstacleAck, CannonShot, InputFrame, Ping, Pong, SupernodeScore};
use std::collections::{HashMap, HashSet};
use std::cell::Cell;
use std::rc::Rc;
use sha2::{Digest, Sha256};
use js_sys;

pub type PeerId = String;

#[derive(Debug, Clone, PartialEq)]
pub enum NetworkState {
    /// Not connected to any room
    Disconnected,
    /// Connecting to signaling server
    Connecting,
    /// Connected, waiting for peers
    WaitingForPeers,
    /// Connected with peers, game in progress
    Connected,
    /// Error occurred
    Error(String),
}

#[derive(Debug, Clone, Default)]
pub struct PlayerStats {
    pub kills: u32,
    pub deaths: u32,
    pub time_played_frames: u32,
}

impl PlayerStats {
    pub fn time_seconds(&self) -> u32 {
        self.time_played_frames / 60
    }
}

pub struct NetworkSession {
    socket: Option<WebRtcSocket>,
    /// Shared flag that gets set to true when the socket loop ends (connection failed)
    socket_closed: Rc<Cell<bool>>,
    pub state: NetworkState,
    pub room_code: String,
    pub local_player_name: String,
    pub remote_players: HashMap<PeerId, RemotePlayer>,
    pub local_peer_id: Option<matchbox_socket::PeerId>,
    pub supernode_id: Option<matchbox_socket::PeerId>,
    pub local_stats: PlayerStats,
    pub remote_stats: HashMap<PeerId, PlayerStats>,
    pending_player_names: HashMap<PeerId, String>,
    peer_id_lookup: HashMap<PeerId, matchbox_socket::PeerId>,
    pending_messages: Vec<(PeerId, NetMessage)>,
    /// Whether this client is the host (room creator) - host controls enemy spawning
    pub is_host: bool,
    /// Received enemy sync from host (for clients)
    pub pending_enemy_sync: Option<EnemySync>,
    /// Received enemy damage events (for host)
    pub pending_enemy_damage: Vec<EnemyDamage>,
    /// Received wave start events (for deterministic spawning)
    pub pending_wave_start: Option<WaveStart>,
    /// Received enemy kill events from other players
    pub pending_enemy_kills: Vec<(PeerId, EnemyKill)>,
    /// Received player death events from other players
    pub pending_player_deaths: Vec<(PeerId, PlayerDeath)>,
    /// Received paid obstacle events from other players
    pub pending_paid_obstacles: Vec<(PeerId, PaidObstacle)>,
    /// Received paid obstacle verification acks
    pub pending_paid_obstacle_acks: Vec<(PeerId, PaidObstacleAck)>,
    /// Received cannon shot events from host
    pub pending_cannon_shots: Vec<CannonShot>,
    /// Received input frames (future rollback netcode)
    pub pending_input_frames: Vec<(PeerId, InputFrame)>,
    /// Latency samples to peers (ms)
    pub latency_ms: HashMap<matchbox_socket::PeerId, u32>,
    /// RTT sample counts per peer
    latency_samples: HashMap<matchbox_socket::PeerId, u8>,
    /// Supernode score reports (score, sample_count, frame_received)
    pub supernode_scores: HashMap<matchbox_socket::PeerId, (u32, u8, u32)>,
    bad_supernodes: HashSet<matchbox_socket::PeerId>,
    last_enemy_sync_frame: u32,
    paid_obstacle_confirmations: HashMap<[u8; 32], HashSet<matchbox_socket::PeerId>>,
    last_ping_frame: u32,
    last_score_frame: u32,
    /// Newly connected peers that need current game state (for late joiners)
    pub new_peers_needing_state: Vec<matchbox_socket::PeerId>,
}

impl NetworkSession {
    pub fn new() -> Self {
        Self {
            socket: None,
            socket_closed: Rc::new(Cell::new(false)),
            state: NetworkState::Disconnected,
            room_code: String::new(),
            local_player_name: Self::generate_default_name(),
            remote_players: HashMap::new(),
            local_peer_id: None,
            supernode_id: None,
            local_stats: PlayerStats::default(),
            remote_stats: HashMap::new(),
            pending_player_names: HashMap::new(),
            peer_id_lookup: HashMap::new(),
            pending_messages: Vec::new(),
            is_host: false,
            pending_enemy_sync: None,
            pending_enemy_damage: Vec::new(),
            pending_wave_start: None,
            pending_enemy_kills: Vec::new(),
            pending_player_deaths: Vec::new(),
            pending_paid_obstacles: Vec::new(),
            pending_paid_obstacle_acks: Vec::new(),
            pending_cannon_shots: Vec::new(),
            pending_input_frames: Vec::new(),
            latency_ms: HashMap::new(),
            latency_samples: HashMap::new(),
            supernode_scores: HashMap::new(),
            bad_supernodes: HashSet::new(),
            last_enemy_sync_frame: 0,
            paid_obstacle_confirmations: HashMap::new(),
            last_ping_frame: 0,
            last_score_frame: 0,
            new_peers_needing_state: Vec::new(),
        }
    }

    /// Generate a random default player name
    fn generate_default_name() -> String {
        use rand::Rng;
        let adjectives = ["Swift", "Brave", "Sly", "Bold", "Keen", "Wild", "Cool", "Rad"];
        let nouns = ["Slime", "Blob", "Goo", "Ooze", "Jelly", "Glob", "Puddle", "Drop"];
        let mut rng = rand::thread_rng();
        let adj = adjectives[rng.gen_range(0..adjectives.len())];
        let noun = nouns[rng.gen_range(0..nouns.len())];
        let num: u16 = rng.gen_range(0..1000);
        format!("{}{}{}", adj, noun, num)
    }

    /// Generate a random room code
    fn generate_room_code() -> String {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        let chars: Vec<char> = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789".chars().collect();
        (0..6).map(|_| chars[rng.gen_range(0..chars.len())]).collect()
    }

    /// Set the local player's name
    pub fn set_player_name(&mut self, name: &str) {
        self.local_player_name = if name.is_empty() {
            Self::generate_default_name()
        } else {
            name.chars().take(32).collect()
        };
    }

    /// Create a new room and return the room code
    /// The creator becomes the host and controls enemy spawning
    pub fn create_room(&mut self, signaling_server: &str) -> String {
        self.room_code = Self::generate_room_code();
        self.is_host = true; // Room creator is the host
        let room_code = self.room_code.clone();
        self.connect(signaling_server, &room_code);
        self.room_code.clone()
    }

    /// Join an existing room by code
    pub fn join_room(&mut self, signaling_server: &str, room_code: &str) {
        self.room_code = room_code.to_uppercase();
        self.is_host = false; // Joiners are not hosts
        let room_code = self.room_code.clone();
        self.connect(signaling_server, &room_code);
    }

    fn connect(&mut self, signaling_server: &str, room_code: &str) {
        // Use game-specific room prefix to avoid conflicts with other matchbox games
        // ?next=2 tells matchbox to start handshake when 2 peers connect
        let room_url = format!("{}/slime_armies_{}?next=2", signaling_server, room_code);

        web_sys::console::log_1(&format!("Connecting to room: {}", room_url).into());

        self.state = NetworkState::Connecting;

        // Reset the closed flag for new connection
        self.socket_closed = Rc::new(Cell::new(false));
        let closed_flag = Rc::clone(&self.socket_closed);

        // WebRtcSocket::new_reliable returns (socket, loop_future) directly, not a Result
        let (socket, loop_fut) = WebRtcSocket::new_reliable(&room_url);
        self.socket = Some(socket);
        self.state = NetworkState::WaitingForPeers;

        // Spawn the socket loop - when it completes, set the closed flag
        wasm_bindgen_futures::spawn_local(async move {
            let _ = loop_fut.await;
            // Socket loop ended - connection failed or closed
            web_sys::console::log_1(&"WebSocket connection closed".into());
            closed_flag.set(true);
        });
    }

    /// Disconnect from the current room
    pub fn disconnect(&mut self) {
        self.socket = None;
        self.state = NetworkState::Disconnected;
        self.room_code.clear();
        self.remote_players.clear();
        self.local_peer_id = None;
        self.supernode_id = None;
        self.pending_player_names.clear();
        self.peer_id_lookup.clear();
        self.bad_supernodes.clear();
        self.last_enemy_sync_frame = 0;
        self.paid_obstacle_confirmations.clear();
        self.latency_ms.clear();
        self.latency_samples.clear();
        self.supernode_scores.clear();
    }

    /// Poll for network events and update state
    /// Returns true if update succeeded, false if connection failed
    pub fn update(&mut self, current_frame: u32) -> bool {
        // Check if socket loop has ended (connection failed)
        if self.socket_closed.get() {
            web_sys::console::log_1(&"Network connection closed".into());
            self.socket = None;
            self.state = NetworkState::Error("Connection failed".to_string());
            return false;
        }

        let (local_id, connected_peers) = {
            let socket = match &mut self.socket {
                Some(s) => s,
                None => return true, // No socket is fine, just nothing to update
            };

            // Try to update peers - this is safe now because we check socket_closed first
            let peers = socket.update_peers();

            // Check for new peers
            let local_name = self.local_player_name.clone();
            for (peer_id, peer_state) in peers {
                let peer_id_str = format!("{:?}", peer_id);
                match peer_state {
                PeerState::Connected => {
                    web_sys::console::log_1(&format!("Peer connected: {}", peer_id_str).into());
                    self.state = NetworkState::Connected;
                    self.peer_id_lookup.insert(peer_id_str.clone(), peer_id);
                    // Send join message with our name
                    let msg = NetMessage::PlayerJoined(local_name.clone()).to_bytes();
                    socket.send(msg.into_boxed_slice(), peer_id);
                    // Track new peer so the elected supernode can send state
                    self.new_peers_needing_state.push(peer_id);
                    }
                PeerState::Disconnected => {
                    web_sys::console::log_1(&format!("Peer disconnected: {}", peer_id_str).into());
                    self.remote_players.remove(&peer_id_str);
                    self.pending_player_names.remove(&peer_id_str);
                    let removed_peer = self.peer_id_lookup.remove(&peer_id_str);
                    if let Some(peer_id) = removed_peer {
                        self.bad_supernodes.remove(&peer_id);
                        self.supernode_scores.remove(&peer_id);
                        self.latency_ms.remove(&peer_id);
                        self.latency_samples.remove(&peer_id);
                    }
                    if self.remote_players.is_empty() {
                        self.state = NetworkState::WaitingForPeers;
                    }
                }
                }
            }

            // Receive messages - safe because we already checked socket_closed
            let messages = socket.receive();

            for (peer_id, data) in messages {
                let peer_id_str = format!("{:?}", peer_id);
                self.peer_id_lookup.insert(peer_id_str.clone(), peer_id);
                if let Some(msg) = NetMessage::from_bytes(&data) {
                    self.pending_messages.push((peer_id_str, msg));
                }
            }

            let local_id = socket.id();
            let connected_peers: Vec<_> = socket.connected_peers().collect();
            (local_id, connected_peers)
        };

        // Process pending messages
        let mut ping_replies: Vec<(PeerId, Ping)> = Vec::new();
        let mut pong_updates: Vec<(PeerId, Pong)> = Vec::new();
        let mut score_updates: Vec<(PeerId, SupernodeScore)> = Vec::new();
        let mut enemy_syncs: Vec<(PeerId, EnemySync)> = Vec::new();
        let mut wave_starts: Vec<(PeerId, WaveStart)> = Vec::new();
        let mut enemy_kills: Vec<(PeerId, EnemyKill)> = Vec::new();
        let mut player_deaths: Vec<(PeerId, PlayerDeath)> = Vec::new();
        let mut paid_obstacles: Vec<(PeerId, PaidObstacle)> = Vec::new();

        for (peer_id, msg) in self.pending_messages.drain(..) {
            match msg {
                NetMessage::PlayerUpdate(state) => {
                    if let Some(remote) = self.remote_players.get_mut(&peer_id) {
                        remote.update_state(&state, current_frame);
                    } else {
                        // New player - add them with current frame to prevent immediate stale removal
                        let name = self.pending_player_names.remove(&peer_id).unwrap_or_else(|| "Player".to_string());
                        self.remote_players.insert(peer_id.clone(), RemotePlayer::new(name, &state, current_frame));
                    }
                }
                NetMessage::PlayerJoined(name) => {
                    web_sys::console::log_1(&format!("Player joined: {} ({})", name, peer_id).into());
                    // Update the player's name if they already exist
                    if let Some(remote) = self.remote_players.get_mut(&peer_id) {
                        remote.name = name;
                    } else {
                        self.pending_player_names.insert(peer_id, name);
                    }
                    // If they don't exist yet, they'll be added on the first PlayerUpdate
                }
                NetMessage::PlayerLeft => {
                    self.remote_players.remove(&peer_id);
                }
                NetMessage::EnemySync(sync) => {
                    enemy_syncs.push((peer_id, sync));
                }
                NetMessage::EnemyDamageEvent(damage) => {
                    // Only accept enemy damage if we're the host
                    if self.is_host {
                        self.pending_enemy_damage.push(damage);
                    }
                }
                NetMessage::WaveStartEvent(wave_start) => {
                    wave_starts.push((peer_id, wave_start));
                }
                NetMessage::EnemyKillEvent(kill) => {
                    enemy_kills.push((peer_id, kill));
                }
                NetMessage::PlayerDeathEvent(death) => {
                    player_deaths.push((peer_id, death));
                }
                NetMessage::PaidObstacleEvent(obstacle) => {
                    paid_obstacles.push((peer_id, obstacle));
                }
                NetMessage::PaidObstacleSyncEvent(sync) => {
                    for obstacle in sync.obstacles {
                        self.pending_paid_obstacles.push(("sync".to_string(), obstacle));
                    }
                }
                NetMessage::PaidObstacleAckEvent(ack) => {
                    self.pending_paid_obstacle_acks.push((peer_id, ack));
                }
                NetMessage::CannonShotEvent(shot) => {
                    self.pending_cannon_shots.push(shot);
                }
                NetMessage::InputFrameEvent(frame) => {
                    self.pending_input_frames.push((peer_id, frame));
                }
                NetMessage::PingEvent(ping) => {
                    ping_replies.push((peer_id, ping));
                }
                NetMessage::PongEvent(pong) => {
                    pong_updates.push((peer_id, pong));
                }
                NetMessage::SupernodeScoreEvent(score) => {
                    score_updates.push((peer_id, score));
                }
            }
        }

        for (peer_id, ping) in ping_replies {
            self.reply_pong(&peer_id, ping);
        }
        for (peer_id, pong) in pong_updates {
            self.record_rtt(&peer_id, pong);
        }
        for (peer_id, score) in score_updates {
            if let Some(peer_id) = self.peer_id_lookup.get(&peer_id) {
                self.supernode_scores.insert(*peer_id, (score.score_ms, score.sample_count, current_frame));
            }
        }
        for (peer_id, sync) in enemy_syncs {
            if !self.is_host && self.is_authoritative_sender(&peer_id) {
                self.pending_enemy_sync = Some(sync);
            } else if !self.is_host {
                web_sys::console::log_1(&format!("Dropped enemy sync from non-authoritative {}", peer_id).into());
            }
        }
        for (peer_id, wave_start) in wave_starts {
            if !self.is_host && self.is_authoritative_sender(&peer_id) {
                self.pending_wave_start = Some(wave_start);
            } else if !self.is_host {
                web_sys::console::log_1(&format!("Dropped wave start from non-authoritative {}", peer_id).into());
            }
        }
        for (peer_id, kill) in enemy_kills {
            if self.is_host {
                self.pending_enemy_kills.push((peer_id.clone(), kill));
                self.relay_enemy_kill(kill);
            } else if self.is_authoritative_sender(&peer_id) {
                self.pending_enemy_kills.push((peer_id, kill));
            }
        }
        for (peer_id, death) in player_deaths {
            if self.is_host {
                self.pending_player_deaths.push((peer_id.clone(), death));
                self.relay_player_death(death);
            } else if self.is_authoritative_sender(&peer_id) {
                self.pending_player_deaths.push((peer_id, death));
            }
        }
        for (peer_id, obstacle) in paid_obstacles {
            if self.is_host {
                self.pending_paid_obstacles.push((peer_id.clone(), obstacle));
                self.relay_paid_obstacle(obstacle);
            } else if self.is_authoritative_sender(&peer_id) {
                self.pending_paid_obstacles.push((peer_id, obstacle));
            }
        }

        // Update interpolation for all remote players
        for remote in self.remote_players.values_mut() {
            remote.update();
        }

        // Remove stale players
        self.remote_players.retain(|_, p| !p.is_stale(current_frame));
        self.remote_stats
            .retain(|peer_id, _| self.remote_players.contains_key(peer_id));
        self.pending_player_names
            .retain(|peer_id, _| !self.remote_players.contains_key(peer_id));

        let prev_supernode = self.supernode_id;
        self.update_supernode_from(local_id, &connected_peers, current_frame);
        if prev_supernode != self.supernode_id {
            web_sys::console::log_1(&format!("Supernode updated: {:?} -> {:?}", prev_supernode, self.supernode_id).into());
        }

        if !connected_peers.is_empty() {
            self.state = NetworkState::Connected;
        } else if self.remote_players.is_empty() {
            self.state = NetworkState::WaitingForPeers;
        }
        self.tick_latency(current_frame, &connected_peers);

        true
    }

    fn update_supernode_from(&mut self, local_id: Option<matchbox_socket::PeerId>, connected_peers: &[matchbox_socket::PeerId], current_frame: u32) {
        if let Some(id) = local_id {
            self.local_peer_id = Some(id);
        }

        let local_id = match self.local_peer_id {
            Some(id) => id,
            None => return,
        };

        let mut candidates: Vec<matchbox_socket::PeerId> = connected_peers
            .iter()
            .copied()
            .filter(|peer_id| !self.bad_supernodes.contains(peer_id))
            .collect();
        candidates.push(local_id);
        if candidates.is_empty() {
            return;
        }

        self.supernode_scores.retain(|peer_id, _| connected_peers.contains(peer_id));
        self.latency_ms.retain(|peer_id, _| connected_peers.contains(peer_id));
        self.latency_samples.retain(|peer_id, _| connected_peers.contains(peer_id));

        let required_samples = (connected_peers.len() as u8).min(3);
        let mut scored: Vec<(matchbox_socket::PeerId, u32)> = Vec::new();

        let local_samples = connected_peers
            .iter()
            .map(|peer_id| *self.latency_samples.get(peer_id).unwrap_or(&0))
            .min()
            .unwrap_or(0);
        if local_samples >= required_samples {
            let score: u32 = connected_peers.iter().map(|peer_id| *self.latency_ms.get(peer_id).unwrap_or(&0)).sum();
            scored.push((local_id, score));
        }

        for peer_id in connected_peers {
            if let Some((score, samples, frame)) = self.supernode_scores.get(peer_id) {
                if *samples >= required_samples
                    && current_frame.saturating_sub(*frame) < 600
                    && !self.bad_supernodes.contains(peer_id)
                {
                    scored.push((*peer_id, *score));
                }
            }
        }

        let have_full_scores = connected_peers
            .iter()
            .all(|peer_id| self.supernode_scores.contains_key(peer_id))
            && local_samples >= required_samples;

        let total_nodes = connected_peers.len() + 1;
        if total_nodes <= 2 {
            let mut all_nodes = connected_peers.to_vec();
            all_nodes.push(local_id);
            all_nodes.sort();
            self.supernode_id = Some(all_nodes[0]);
            self.is_host = all_nodes[0] == local_id;
            return;
        }

        let supernode = if let Some(current_supernode) = self.supernode_id {
            if connected_peers.contains(&current_supernode)
                && !self.bad_supernodes.contains(&current_supernode)
                && (current_supernode == local_id || current_frame.saturating_sub(self.last_enemy_sync_frame) < 180)
            {
                current_supernode
            } else if have_full_scores && !scored.is_empty() {
                scored.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
                scored[0].0
            } else {
                candidates.sort();
                candidates[0]
            }
        } else if have_full_scores && !scored.is_empty() {
            scored.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
            scored[0].0
        } else {
            candidates.sort();
            candidates[0]
        };
        self.supernode_id = Some(supernode);
        self.is_host = supernode == local_id;
    }

    pub fn mark_enemy_sync_received(&mut self, current_frame: u32) {
        self.last_enemy_sync_frame = current_frame;
    }

    pub fn supernode_is_stale(&self, current_frame: u32) -> bool {
        current_frame.saturating_sub(self.last_enemy_sync_frame) >= 180
    }

    pub fn mark_supernode_bad(&mut self, current_frame: u32) {
        if let Some(supernode) = self.supernode_id {
            self.bad_supernodes.insert(supernode);
        }
        self.last_enemy_sync_frame = current_frame.saturating_sub(1000);
    }

    pub fn is_supernode_sender(&self, peer_id: &str) -> bool {
        match (self.supernode_id, self.peer_id_lookup.get(peer_id)) {
            (Some(supernode), Some(sender)) => *sender == supernode,
            _ => false,
        }
    }

    fn is_authoritative_sender(&self, peer_id: &str) -> bool {
        self.supernode_id.is_none() || self.is_supernode_sender(peer_id)
    }

    pub fn record_paid_obstacle_confirmation(&mut self, proof_hash: [u8; 32], peer_id: matchbox_socket::PeerId) -> usize {
        let entry = self.paid_obstacle_confirmations.entry(proof_hash).or_default();
        entry.insert(peer_id);
        entry.len()
    }

    pub fn paid_obstacle_confirmation_count(&self, proof_hash: [u8; 32]) -> usize {
        self.paid_obstacle_confirmations
            .get(&proof_hash)
            .map(|set| set.len())
            .unwrap_or(0)
    }

    pub fn resolve_peer_id(&self, peer_id: &str) -> Option<matchbox_socket::PeerId> {
        self.peer_id_lookup.get(peer_id).copied()
    }

    fn tick_latency(&mut self, current_frame: u32, connected_peers: &[matchbox_socket::PeerId]) {
        let socket = match &mut self.socket {
            Some(s) => s,
            None => return,
        };

        if current_frame.saturating_sub(self.last_ping_frame) >= 120 {
            self.last_ping_frame = current_frame;
            let now_ms = js_sys::Date::now() as u64 as u32;
            let ping = Ping { nonce: current_frame, sent_ms: now_ms };
            let msg = NetMessage::PingEvent(ping).to_bytes();
            for peer_id in connected_peers {
                socket.send(msg.clone().into_boxed_slice(), *peer_id);
            }
        }

        if current_frame.saturating_sub(self.last_score_frame) >= 180 {
            self.last_score_frame = current_frame;
            let sample_count = connected_peers
                .iter()
                .map(|peer_id| *self.latency_samples.get(peer_id).unwrap_or(&0))
                .min()
                .unwrap_or(0);
            if sample_count > 0 {
                let score_ms: u32 = connected_peers.iter().map(|peer_id| *self.latency_ms.get(peer_id).unwrap_or(&1000)).sum();
                let score = SupernodeScore { score_ms, sample_count };
                let msg = NetMessage::SupernodeScoreEvent(score).to_bytes();
                for peer_id in connected_peers {
                    socket.send(msg.clone().into_boxed_slice(), *peer_id);
                }
            }
        }
    }

    fn reply_pong(&mut self, peer_id: &str, ping: Ping) {
        let socket = match &mut self.socket {
            Some(s) => s,
            None => return,
        };

        if let Some(peer_id) = self.peer_id_lookup.get(peer_id) {
            let pong = Pong { nonce: ping.nonce, sent_ms: ping.sent_ms };
            let msg = NetMessage::PongEvent(pong).to_bytes();
            socket.send(msg.into_boxed_slice(), *peer_id);
        }
    }

    fn record_rtt(&mut self, peer_id: &str, pong: Pong) {
        if let Some(peer_id) = self.peer_id_lookup.get(peer_id) {
            let now_ms = js_sys::Date::now() as u64 as u32;
            let rtt = now_ms.wrapping_sub(pong.sent_ms);
            self.latency_ms.insert(*peer_id, rtt);
            let samples = self.latency_samples.entry(*peer_id).or_insert(0);
            *samples = samples.saturating_add(1);
        }
    }

    pub fn reset_stats(&mut self) {
        self.local_stats = PlayerStats::default();
        self.remote_stats.clear();
    }

    pub fn tick_playtime(&mut self, in_game: bool) {
        if !in_game {
            return;
        }

        self.local_stats.time_played_frames = self.local_stats.time_played_frames.saturating_add(1);
        for peer_id in self.remote_players.keys() {
            let stats = self.remote_stats.entry(peer_id.clone()).or_default();
            stats.time_played_frames = stats.time_played_frames.saturating_add(1);
        }
    }

    pub fn record_local_kills(&mut self, count: u32) {
        self.local_stats.kills = self.local_stats.kills.saturating_add(count);
    }

    pub fn record_local_deaths(&mut self, count: u32) {
        self.local_stats.deaths = self.local_stats.deaths.saturating_add(count);
    }

    pub fn record_remote_kill(&mut self, peer_id: &PeerId, count: u32) {
        let stats = self.remote_stats.entry(peer_id.clone()).or_default();
        stats.kills = stats.kills.saturating_add(count);
    }

    pub fn record_remote_death(&mut self, peer_id: &PeerId, count: u32) {
        let stats = self.remote_stats.entry(peer_id.clone()).or_default();
        stats.deaths = stats.deaths.saturating_add(count);
    }

    pub fn room_totals(&self) -> PlayerStats {
        let mut totals = self.local_stats.clone();
        for stats in self.remote_stats.values() {
            totals.kills = totals.kills.saturating_add(stats.kills);
            totals.deaths = totals.deaths.saturating_add(stats.deaths);
            totals.time_played_frames = totals.time_played_frames.saturating_add(stats.time_played_frames);
        }
        totals
    }

    /// Send local player state to all peers
    pub fn send_player_state(&mut self, state: PlayerState) {
        let socket = match &mut self.socket {
            Some(s) => s,
            None => return,
        };

        let msg = NetMessage::PlayerUpdate(state).to_bytes();
        let peers: Vec<_> = socket.connected_peers().collect();

        for peer_id in peers {
            socket.send(msg.clone().into_boxed_slice(), peer_id);
        }
    }

    pub fn apply_predicted_states(&mut self, predictions: &std::collections::HashMap<String, PlayerState>) {
        for (peer_id, state) in predictions {
            if let Some(remote) = self.remote_players.get_mut(peer_id) {
                remote.apply_predicted_state(state);
            }
        }
    }

    /// Check if we have any connected peers
    pub fn has_peers(&self) -> bool {
        !self.remote_players.is_empty()
    }

    /// Get number of connected peers
    pub fn peer_count(&self) -> usize {
        self.remote_players.len()
    }

    /// Send enemy sync to all peers (host only)
    pub fn send_enemy_sync(&mut self, sync: EnemySync) {
        if !self.is_host {
            return;
        }

        let socket = match &mut self.socket {
            Some(s) => s,
            None => return,
        };

        let msg = NetMessage::EnemySync(sync).to_bytes();
        let peers: Vec<_> = socket.connected_peers().collect();

        for peer_id in peers {
            socket.send(msg.clone().into_boxed_slice(), peer_id);
        }
    }

    fn relay_enemy_kill(&mut self, kill: EnemyKill) {
        let socket = match &mut self.socket {
            Some(s) => s,
            None => return,
        };

        let msg = NetMessage::EnemyKillEvent(kill).to_bytes();
        let peers: Vec<_> = socket.connected_peers().collect();

        for peer_id in peers {
            socket.send(msg.clone().into_boxed_slice(), peer_id);
        }
    }

    fn relay_player_death(&mut self, death: PlayerDeath) {
        let socket = match &mut self.socket {
            Some(s) => s,
            None => return,
        };

        let msg = NetMessage::PlayerDeathEvent(death).to_bytes();
        let peers: Vec<_> = socket.connected_peers().collect();

        for peer_id in peers {
            socket.send(msg.clone().into_boxed_slice(), peer_id);
        }
    }

    fn relay_paid_obstacle(&mut self, obstacle: PaidObstacle) {
        let socket = match &mut self.socket {
            Some(s) => s,
            None => return,
        };

        let msg = NetMessage::PaidObstacleEvent(obstacle).to_bytes();
        let peers: Vec<_> = socket.connected_peers().collect();

        for peer_id in peers {
            socket.send(msg.clone().into_boxed_slice(), peer_id);
        }
    }

    /// Send enemy damage event to host (client only)
    pub fn send_enemy_damage(&mut self, damage: EnemyDamage) {
        if self.is_host {
            // Host processes locally, no need to send
            self.pending_enemy_damage.push(damage);
            return;
        }

        let socket = match &mut self.socket {
            Some(s) => s,
            None => return,
        };

        let msg = NetMessage::EnemyDamageEvent(damage).to_bytes();
        let peers: Vec<_> = socket.connected_peers().collect();

        // Send to all peers (only host will process it)
        if let Some(supernode) = self.supernode_id {
            socket.send(msg.into_boxed_slice(), supernode);
        } else {
            for peer_id in peers {
                socket.send(msg.clone().into_boxed_slice(), peer_id);
            }
        }
    }

    /// Take pending enemy sync (for client to apply)
    pub fn take_enemy_sync(&mut self) -> Option<EnemySync> {
        self.pending_enemy_sync.take()
    }

    /// Take pending enemy damage events (for host to process)
    pub fn take_enemy_damage(&mut self) -> Vec<EnemyDamage> {
        std::mem::take(&mut self.pending_enemy_damage)
    }

    /// Send wave start event to all peers (host only)
    pub fn send_wave_start(&mut self, wave_start: WaveStart) {
        if !self.is_host {
            return;
        }

        let socket = match &mut self.socket {
            Some(s) => s,
            None => return,
        };

        let msg = NetMessage::WaveStartEvent(wave_start).to_bytes();
        let peers: Vec<_> = socket.connected_peers().collect();

        for peer_id in peers {
            socket.send(msg.clone().into_boxed_slice(), peer_id);
        }
    }

    /// Send enemy kill event to all peers
    pub fn send_enemy_kill(&mut self, kill: EnemyKill) {
        let socket = match &mut self.socket {
            Some(s) => s,
            None => return,
        };

        let msg = NetMessage::EnemyKillEvent(kill).to_bytes();
        if self.is_host {
            let peers: Vec<_> = socket.connected_peers().collect();
            for peer_id in peers {
                socket.send(msg.clone().into_boxed_slice(), peer_id);
            }
        } else if let Some(supernode) = self.supernode_id {
            socket.send(msg.into_boxed_slice(), supernode);
        }
    }

    /// Send player death event to all peers
    pub fn send_player_death(&mut self, death: PlayerDeath) {
        let socket = match &mut self.socket {
            Some(s) => s,
            None => return,
        };

        let msg = NetMessage::PlayerDeathEvent(death).to_bytes();
        if self.is_host {
            let peers: Vec<_> = socket.connected_peers().collect();
            for peer_id in peers {
                socket.send(msg.clone().into_boxed_slice(), peer_id);
            }
        } else if let Some(supernode) = self.supernode_id {
            socket.send(msg.into_boxed_slice(), supernode);
        }
    }

    /// Send paid obstacle event to all peers
    pub fn send_paid_obstacle(&mut self, obstacle: PaidObstacle) {
        let socket = match &mut self.socket {
            Some(s) => s,
            None => return,
        };

        let msg = NetMessage::PaidObstacleEvent(obstacle).to_bytes();
        let peers: Vec<_> = socket.connected_peers().collect();

        for peer_id in peers {
            socket.send(msg.clone().into_boxed_slice(), peer_id);
        }
    }

    pub fn send_paid_obstacle_to_supernode(&mut self, obstacle: PaidObstacle) {
        if let (Some(supernode), Some(socket)) = (self.supernode_id, self.socket.as_mut()) {
            let msg = NetMessage::PaidObstacleEvent(obstacle).to_bytes();
            socket.send(msg.into_boxed_slice(), supernode);
        }
    }

    pub fn send_paid_obstacle_ack(&mut self, ack: PaidObstacleAck) {
        let socket = match &mut self.socket {
            Some(s) => s,
            None => return,
        };

        let msg = NetMessage::PaidObstacleAckEvent(ack).to_bytes();
        let peers: Vec<_> = socket.connected_peers().collect();

        for peer_id in peers {
            socket.send(msg.clone().into_boxed_slice(), peer_id);
        }
    }

    pub fn send_cannon_shot(&mut self, shot: CannonShot) {
        if !self.is_host {
            return;
        }

        let socket = match &mut self.socket {
            Some(s) => s,
            None => return,
        };

        let msg = NetMessage::CannonShotEvent(shot).to_bytes();
        let peers: Vec<_> = socket.connected_peers().collect();

        for peer_id in peers {
            socket.send(msg.clone().into_boxed_slice(), peer_id);
        }
    }

    pub fn send_input_frame(&mut self, frame: InputFrame) {
        let socket = match &mut self.socket {
            Some(s) => s,
            None => return,
        };

        let msg = NetMessage::InputFrameEvent(frame).to_bytes();
        let peers: Vec<_> = socket.connected_peers().collect();

        for peer_id in peers {
            socket.send(msg.clone().into_boxed_slice(), peer_id);
        }
    }

    /// Take pending wave start (for client to apply)
    pub fn take_wave_start(&mut self) -> Option<WaveStart> {
        self.pending_wave_start.take()
    }

    /// Take pending enemy kills from other players
    pub fn take_enemy_kills(&mut self) -> Vec<(PeerId, EnemyKill)> {
        std::mem::take(&mut self.pending_enemy_kills)
    }

    /// Take pending player deaths from other players
    pub fn take_player_deaths(&mut self) -> Vec<(PeerId, PlayerDeath)> {
        std::mem::take(&mut self.pending_player_deaths)
    }

    /// Take pending paid obstacles from other players
    pub fn take_paid_obstacles(&mut self) -> Vec<(PeerId, PaidObstacle)> {
        std::mem::take(&mut self.pending_paid_obstacles)
    }

    pub fn take_paid_obstacle_acks(&mut self) -> Vec<(PeerId, PaidObstacleAck)> {
        std::mem::take(&mut self.pending_paid_obstacle_acks)
    }

    pub fn take_cannon_shots(&mut self) -> Vec<CannonShot> {
        std::mem::take(&mut self.pending_cannon_shots)
    }

    pub fn take_input_frames(&mut self) -> Vec<(PeerId, InputFrame)> {
        std::mem::take(&mut self.pending_input_frames)
    }

    /// Send wave start to specific peers (for late joiners)
    pub fn send_wave_start_to_peers(&mut self, wave_start: &WaveStart, peers: &[matchbox_socket::PeerId]) {
        if !self.is_host || peers.is_empty() {
            return;
        }

        let socket = match &mut self.socket {
            Some(s) => s,
            None => return,
        };

        let msg = NetMessage::WaveStartEvent(*wave_start).to_bytes();

        for peer_id in peers {
            socket.send(msg.clone().into_boxed_slice(), *peer_id);
            web_sys::console::log_1(&format!("Sent wave state to late joiner: {:?}", peer_id).into());
        }
    }

    /// Send paid obstacle sync to specific peers (for late joiners)
    pub fn send_paid_obstacles_to_peers(&mut self, obstacles: &[PaidObstacle], peers: &[matchbox_socket::PeerId]) {
        if !self.is_host || peers.is_empty() || obstacles.is_empty() {
            return;
        }

        let socket = match &mut self.socket {
            Some(s) => s,
            None => return,
        };

        let sync = PaidObstacleSync {
            obstacles: obstacles.to_vec(),
        };
        let msg = NetMessage::PaidObstacleSyncEvent(sync).to_bytes();

        for peer_id in peers {
            socket.send(msg.clone().into_boxed_slice(), *peer_id);
            web_sys::console::log_1(&format!("Sent paid obstacles to late joiner: {:?}", peer_id).into());
        }
    }

    pub fn send_paid_obstacles_to_all(&mut self, obstacles: &[PaidObstacle]) {
        if !self.is_host || obstacles.is_empty() {
            return;
        }

        let socket = match &mut self.socket {
            Some(s) => s,
            None => return,
        };

        let sync = PaidObstacleSync {
            obstacles: obstacles.to_vec(),
        };
        let msg = NetMessage::PaidObstacleSyncEvent(sync).to_bytes();
        let peers: Vec<_> = socket.connected_peers().collect();

        for peer_id in peers {
            socket.send(msg.clone().into_boxed_slice(), peer_id);
        }
    }

    pub fn take_new_peers_needing_state(&mut self) -> Vec<matchbox_socket::PeerId> {
        std::mem::take(&mut self.new_peers_needing_state)
    }

    pub fn verify_paid_obstacle(&self, _obstacle: &PaidObstacle) -> bool {
        // TODO: verify on-chain receipt/proof (x402 or token ownership).
        // For now, check hash matches payload + room code; on-chain check still required.
        let expected = Self::compute_paid_obstacle_hash(&self.room_code, _obstacle);
        _obstacle.proof_hash == expected
    }

    fn compute_paid_obstacle_hash(room_code: &str, obstacle: &PaidObstacle) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(room_code.as_bytes());
        hasher.update(&obstacle.x.to_le_bytes());
        hasher.update(&obstacle.y.to_le_bytes());
        hasher.update(&obstacle.radius.to_le_bytes());
        hasher.update(&[obstacle.variant]);
        let digest = hasher.finalize();
        let mut out = [0u8; 32];
        out.copy_from_slice(&digest);
        out
    }

    /// Check if there are new peers that need game state
    pub fn has_new_peers_needing_state(&self) -> bool {
        self.is_host && !self.new_peers_needing_state.is_empty()
    }
}

impl Default for NetworkSession {
    fn default() -> Self {
        Self::new()
    }
}

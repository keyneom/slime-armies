use matchbox_socket::{PeerState, WebRtcSocket};
use crate::net::{NetMessage, PlayerState, RemotePlayer, EnemySync, EnemyDamage, WaveStart, EnemyKill, PlayerDeath, PaidObstacle, PaidObstacleSync, PaidObstacleAck, PaidAbility, PaidAbilityAck, PaidAbilityType, CannonShot, InputFrame, Ping, Pong, SupernodeScore, ChatMessage, VoteMute, PlayerStateBatch, PlayerStateEntry, InputFrameBatch, InputFrameEntry, TopologyUpdate, AreaAuthorityUpdate, AreaAuthorityEntry};
use crate::math::Vec2;
use crate::world::CHUNK_SIZE;
use std::collections::{HashMap, HashSet};
use std::cell::Cell;
use std::rc::Rc;
use sha2::{Digest, Sha256};
use js_sys;

pub type PeerId = String;

#[derive(Debug, Clone)]
pub struct IceConfig {
    pub urls: Vec<String>,
    pub username: Option<String>,
    pub credential: Option<String>,
}

impl Default for IceConfig {
    fn default() -> Self {
        Self {
            urls: vec![
                "stun:stun.l.google.com:19302".to_string(),
                "stun:stun1.l.google.com:19302".to_string(),
            ],
            username: None,
            credential: None,
        }
    }
}

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
    pub spider_kills: u32,
    pub cannon_kills: u32,
    pub snake_kills: u32,
    pub wisp_kills: u32,
    pub attack_attempts: u32,
    pub attack_hits: u32,
    pub deaths: u32,
    pub time_played_frames: u32,
}

impl PlayerStats {
    pub fn time_seconds(&self) -> u32 {
        self.time_played_frames / 60
    }
}

#[derive(Debug, Clone, Default)]
pub struct RelayTelemetry {
    pub recv_messages: u32,
    pub sent_upstream: u32,
    pub sent_downstream: u32,
    pub sent_broadcast: u32,
    pub dropped_messages: u32,
    pub dropped_queue_entries: u32,
    pub max_queue_depth: usize,
    pub stale_parent_switches: u32,
}

#[derive(Debug, Clone, Copy)]
enum LowPriorityTopic {
    Chat,
    Vote,
    Ack,
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
    pub local_peer_hash: Option<u64>,
    pub supernode_id: Option<matchbox_socket::PeerId>,
    pub super_root_id: Option<matchbox_socket::PeerId>,
    pub supernode_set: Vec<matchbox_socket::PeerId>,
    peer_anchor_supernode: HashMap<matchbox_socket::PeerId, matchbox_socket::PeerId>,
    relay_parent: Option<matchbox_socket::PeerId>,
    relay_backup_parent: Option<matchbox_socket::PeerId>,
    relay_active_parent: Option<matchbox_socket::PeerId>,
    relay_fanout: usize,
    relay_children: Vec<matchbox_socket::PeerId>,
    desired_peer_set: HashSet<matchbox_socket::PeerId>,
    discovery_attached: bool,
    discovery_attach_frame: Option<u32>,
    bootstrap_full_mesh_active: bool,
    frames_without_peer_connection: u32,
    relay_epoch: u32,
    last_parent_switch_frame: u32,
    stale_parent_events: u32,
    last_peer_message_frames: HashMap<matchbox_socket::PeerId, u32>,
    pub relay_telemetry: RelayTelemetry,
    last_update_frame: u32,
    last_lowpri_chat_frame: u32,
    last_lowpri_vote_frame: u32,
    last_lowpri_ack_frame: u32,
    last_telemetry_log_frame: u32,
    area_authorities: HashMap<u32, u64>,
    last_topology_broadcast_frame: u32,
    last_area_update_broadcast_frame: u32,
    local_last_pos: Option<Vec2>,
    pub local_stats: PlayerStats,
    pub remote_stats: HashMap<PeerId, PlayerStats>,
    pending_player_names: HashMap<PeerId, String>,
    peer_id_lookup: HashMap<PeerId, matchbox_socket::PeerId>,
    peer_hash_lookup: HashMap<u64, PeerId>,
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
    pub pending_enemy_kills_optimistic: Vec<EnemyKill>,
    pub pending_enemy_kills_confirmed: Vec<EnemyKill>,
    /// Received player death events from other players
    pub pending_player_deaths_optimistic: Vec<PlayerDeath>,
    pub pending_player_deaths_confirmed: Vec<PlayerDeath>,
    enemy_kill_confirmations: HashMap<u64, (EnemyKill, HashSet<PeerId>, u32)>,
    player_death_confirmations: HashMap<u64, (PlayerDeath, HashSet<PeerId>, u32)>,
    applied_event_ids: HashMap<u64, u32>,
    optimistic_enemy_event_ids: HashMap<u64, u32>,
    optimistic_death_event_ids: HashMap<u64, u32>,
    /// Received paid obstacle events from other players
    pub pending_paid_obstacles: Vec<(PeerId, PaidObstacle)>,
    /// Received paid obstacle verification acks
    pub pending_paid_obstacle_acks: Vec<(PeerId, PaidObstacleAck)>,
    /// Received paid ability events from other players
    pub pending_paid_abilities: Vec<(PeerId, PaidAbility)>,
    /// Received paid ability verification acks
    pub pending_paid_ability_acks: Vec<(PeerId, PaidAbilityAck)>,
    /// Received cannon shot events from host
    pub pending_cannon_shots: Vec<CannonShot>,
    /// Received chat messages
    pending_chat_messages: Vec<ChatMessage>,
    /// Received vote mute events that reached threshold
    pending_vote_mutes: Vec<VoteMute>,
    /// Muted sender hashes
    muted_hashes: HashSet<u64>,
    /// Vote mute tracking (target -> voters)
    vote_mutes: HashMap<u64, HashSet<u64>>,
    /// Received input frames (future rollback netcode)
    pub pending_input_frames: Vec<(PeerId, InputFrame)>,
    relay_player_states: HashMap<u64, PlayerState>,
    relay_input_frames: Vec<InputFrameEntry>,
    downlink_player_batches: Vec<PlayerStateBatch>,
    downlink_input_batches: Vec<InputFrameBatch>,
    /// Latency samples to peers (ms)
    pub latency_ms: HashMap<matchbox_socket::PeerId, u32>,
    /// RTT sample counts per peer
    latency_samples: HashMap<matchbox_socket::PeerId, u8>,
    /// Supernode score reports (score, sample_count, frame_received)
    pub supernode_scores: HashMap<matchbox_socket::PeerId, (u32, u8, u32)>,
    bad_supernodes: HashSet<matchbox_socket::PeerId>,
    last_enemy_sync_frame: u32,
    paid_obstacle_confirmations: HashMap<[u8; 32], HashSet<matchbox_socket::PeerId>>,
    paid_ability_confirmations: HashMap<[u8; 32], HashSet<matchbox_socket::PeerId>>,
    last_ping_frame: u32,
    last_score_frame: u32,
    /// Newly connected peers that need current game state (for late joiners)
    pub new_peers_needing_state: Vec<matchbox_socket::PeerId>,
}

impl NetworkSession {
    const MAX_SUPERNODES: usize = 256;
    const MAX_FANOUT: usize = 12;
    const MIN_FANOUT: usize = 2;
    const AREA_GROUP_CHUNKS: i32 = 4;
    const TARGET_PLAYERS_PER_SUPERNODE: usize = 24;
    const TARGET_AREAS_PER_SUPERNODE: usize = 2;
    const ANCHOR_SWITCH_MARGIN: f32 = 55_000.0;
    const ANCHOR_FORCE_DISTANCE_SQ: f32 = 1_100_000.0;
    const RELAY_PARENT_STALE_FRAMES: u32 = 180;
    const RELAY_FAILOVER_COOLDOWN_FRAMES: u32 = 120;
    const RELAY_FAILOVER_MIN_SAMPLES: u8 = 2;
    const RELAY_HANDOFF_DUPLEX_FRAMES: u32 = 24;
    const LEAF_LINK_CAP: usize = 5;
    const SUPERNODE_LINK_CAP: usize = 16;
    const ROOT_LINK_CAP: usize = 14;
    const DISCOVERY_MIN_ATTACH_FRAMES: u32 = 180;
    const BOOTSTRAP_FULLMESH_TRIGGER_FRAMES: u32 = 120;
    const BOOTSTRAP_PROBE_LINKS: usize = 8;
    const MAX_RELAY_INPUT_QUEUE: usize = 1024;
    const MAX_DOWNLINK_QUEUE: usize = 64;
    const MAX_BATCH_ENTRIES: usize = 512;

    fn interest_radius_sq() -> f32 {
        let radius = 1800.0;
        radius * radius
    }

    fn area_id_from_pos(pos: Vec2) -> u32 {
        let area_world = (CHUNK_SIZE as i32 * Self::AREA_GROUP_CHUNKS) as f32;
        let ax = (pos.x / area_world).floor() as i32;
        let ay = (pos.y / area_world).floor() as i32;
        let ux = (ax as i64 - i32::MIN as i64) as u32;
        let uy = (ay as i64 - i32::MIN as i64) as u32;
        ((ux & 0xFFFF) << 16) | (uy & 0xFFFF)
    }

    fn hash_to_matchbox(&self, hash: u64) -> Option<matchbox_socket::PeerId> {
        let peer_id = self.peer_hash_lookup.get(&hash)?;
        self.peer_id_lookup.get(peer_id).copied()
    }

    fn area_authority_for(&self, area_id: u32) -> Option<matchbox_socket::PeerId> {
        let hash = self.area_authorities.get(&area_id).copied()?;
        self.hash_to_matchbox(hash)
    }

    fn area_id_for_hash(&self, peer_hash: u64) -> u32 {
        if Some(peer_hash) == self.local_peer_hash {
            if let Some(pos) = self.local_last_pos {
                return Self::area_id_from_pos(pos);
            }
            return 0;
        }
        if let Some(peer_id) = self.peer_hash_lookup.get(&peer_hash) {
            if let Some(remote) = self.remote_players.get(peer_id) {
                return Self::area_id_from_pos(remote.pos);
            }
        }
        0
    }

    fn peer_pos(&self, peer_id: matchbox_socket::PeerId) -> Option<Vec2> {
        if Some(peer_id) == self.local_peer_id {
            return self.local_last_pos;
        }
        let id = format!("{:?}", peer_id);
        self.remote_players.get(&id).map(|remote| remote.pos)
    }

    fn choose_dynamic_supernode_count(total_nodes: usize, active_areas: usize) -> usize {
        if total_nodes <= 3 {
            return 1;
        }
        let area_count = active_areas.max(1);
        let small_room_floor = match total_nodes {
            0..=3 => 1,
            4..=8 => 2,
            9..=16 => 3,
            17..=24 => 4,
            25..=40 => 6,
            41..=64 => 8,
            _ => 0,
        };
        let by_scale = ((total_nodes as f32).sqrt() / 1.8).ceil() as usize;
        let by_load = total_nodes.div_ceil(Self::TARGET_PLAYERS_PER_SUPERNODE.max(1));
        let by_areas = area_count
            .max(1)
            .div_ceil(Self::TARGET_AREAS_PER_SUPERNODE.max(1));
        let mut target = small_room_floor.max(by_scale).max(by_load).max(by_areas);
        if total_nodes <= 64 {
            // Prevent over-sharding in small rooms.
            target = target.min((total_nodes / 2).max(1));
        }
        if total_nodes >= 5_000 {
            // Keep enough relay heads in very large rooms.
            let huge_floor = ((total_nodes as f32).sqrt() / 1.4).ceil() as usize;
            target = target.max(huge_floor);
        }
        target
            .clamp(1, Self::MAX_SUPERNODES)
            .min(total_nodes)
    }

    fn choose_dynamic_fanout(total_nodes: usize) -> usize {
        let fanout = match total_nodes {
            0..=3 => 2,
            4..=8 => 3,
            9..=16 => 4,
            17..=32 => 5,
            33..=64 => 6,
            65..=128 => 7,
            129..=512 => 8,
            513..=2048 => 9,
            2049..=8192 => 10,
            _ => 12,
        };
        fanout.clamp(Self::MIN_FANOUT, Self::MAX_FANOUT)
    }

    fn select_supernodes_dynamic(
        &self,
        all_nodes: &[matchbox_socket::PeerId],
        target_k: usize,
    ) -> Vec<matchbox_socket::PeerId> {
        let mut ranked = all_nodes.to_vec();
        ranked.sort();
        let mut eligible: Vec<matchbox_socket::PeerId> = ranked
            .iter()
            .copied()
            .filter(|id| !self.bad_supernodes.contains(id))
            .collect();
        if eligible.is_empty() {
            eligible = ranked.clone();
        }
        if eligible.is_empty() {
            return Vec::new();
        }

        let mut area_counts: HashMap<u32, usize> = HashMap::new();
        for node in &eligible {
            let area = self
                .peer_pos(*node)
                .map(Self::area_id_from_pos)
                .unwrap_or(0);
            *area_counts.entry(area).or_insert(0) += 1;
        }

        let mut selected = Vec::new();
        if let Some(root) = self.super_root_id {
            if eligible.contains(&root) {
                selected.push(root);
            }
        }
        if selected.is_empty() {
            selected.push(eligible[0]);
        }

        while selected.len() < target_k {
            let mut best: Option<(matchbox_socket::PeerId, f32)> = None;
            for candidate in &eligible {
                if selected.contains(candidate) {
                    continue;
                }
                let cand_pos = self.peer_pos(*candidate);
                let area = cand_pos.map(Self::area_id_from_pos).unwrap_or(0);
                let area_density = *area_counts.get(&area).unwrap_or(&1) as f32;
                let min_dist = selected
                    .iter()
                    .map(|picked| {
                        let a = cand_pos.unwrap_or(Vec2::ZERO);
                        let b = self.peer_pos(*picked).unwrap_or(Vec2::ZERO);
                        let d = a - b;
                        d.x * d.x + d.y * d.y
                    })
                    .fold(f32::MAX, f32::min);
                let latency = self.latency_ms.get(candidate).copied().unwrap_or(200) as f32;
                let score = area_density * 8_000.0 + min_dist * 0.03 - latency * 80.0;
                match best {
                    Some((id, best_score)) => {
                        if score > best_score || (score == best_score && *candidate < id) {
                            best = Some((*candidate, score));
                        }
                    }
                    None => best = Some((*candidate, score)),
                }
            }
            if let Some((candidate, _)) = best {
                selected.push(candidate);
            } else {
                break;
            }
        }

        selected.sort();
        selected
    }

    fn build_clustered_relay_order(
        &mut self,
        all_nodes: &[matchbox_socket::PeerId],
        supernodes: &[matchbox_socket::PeerId],
        super_root: matchbox_socket::PeerId,
    ) -> Vec<matchbox_socket::PeerId> {
        if supernodes.is_empty() {
            return all_nodes.to_vec();
        }

        let root = super_root;
        let root_pos = self.peer_pos(root).unwrap_or(Vec2::ZERO);

        let mut backbone: Vec<matchbox_socket::PeerId> = supernodes
            .iter()
            .copied()
            .filter(|id| *id != root)
            .collect();
        backbone.sort_by(|a, b| {
            let pa = self.peer_pos(*a).unwrap_or(root_pos);
            let pb = self.peer_pos(*b).unwrap_or(root_pos);
            let da = {
                let d = pa - root_pos;
                d.x * d.x + d.y * d.y
            };
            let db = {
                let d = pb - root_pos;
                d.x * d.x + d.y * d.y
            };
            da.partial_cmp(&db)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.cmp(b))
        });

        let mut anchors: HashMap<matchbox_socket::PeerId, Vec<matchbox_socket::PeerId>> = HashMap::new();
        let mut loads: HashMap<matchbox_socket::PeerId, usize> = HashMap::new();
        for supernode in supernodes {
            anchors.insert(*supernode, Vec::new());
            loads.insert(*supernode, 0);
            self.peer_anchor_supernode.insert(*supernode, *supernode);
        }

        let mut leaves: Vec<matchbox_socket::PeerId> = all_nodes
            .iter()
            .copied()
            .filter(|id| !supernodes.contains(id))
            .collect();
        leaves.sort();

        for node in leaves {
            let node_pos = self.peer_pos(node).unwrap_or(Vec2::ZERO);
            let mut best: Option<(matchbox_socket::PeerId, f32)> = None;
            let mut scores: HashMap<matchbox_socket::PeerId, f32> = HashMap::new();
            for supernode in supernodes {
                let super_pos = self.peer_pos(*supernode).unwrap_or(node_pos);
                let d = super_pos - node_pos;
                let distance_score = d.x * d.x + d.y * d.y;
                let latency_score = self.latency_ms.get(supernode).copied().unwrap_or(200) as f32 * 220.0;
                let load_score = *loads.get(supernode).unwrap_or(&0) as f32 * 18_000.0;
                let score = distance_score + latency_score + load_score;
                scores.insert(*supernode, score);
                match best {
                    Some((id, best_score)) => {
                        if score < best_score || (score == best_score && *supernode < id) {
                            best = Some((*supernode, score));
                        }
                    }
                    None => best = Some((*supernode, score)),
                }
            }
            let mut chosen = best.map(|(id, _)| id).unwrap_or(root);
            if let Some(prev_anchor) = self.peer_anchor_supernode.get(&node).copied() {
                if prev_anchor != chosen && supernodes.contains(&prev_anchor) {
                    let prev_score = scores.get(&prev_anchor).copied().unwrap_or(f32::MAX);
                    let chosen_score = scores.get(&chosen).copied().unwrap_or(prev_score);
                    let prev_pos = self.peer_pos(prev_anchor).unwrap_or(node_pos);
                    let pd = prev_pos - node_pos;
                    let prev_dist_sq = pd.x * pd.x + pd.y * pd.y;
                    let switch_is_decisive = chosen_score + Self::ANCHOR_SWITCH_MARGIN < prev_score;
                    let force_switch = prev_dist_sq > Self::ANCHOR_FORCE_DISTANCE_SQ;
                    if !switch_is_decisive && !force_switch {
                        chosen = prev_anchor;
                    }
                }
            }
            self.peer_anchor_supernode.insert(node, chosen);
            *loads.entry(chosen).or_insert(0) += 1;
            anchors.entry(chosen).or_default().push(node);
        }

        let mut relay_order = vec![root];
        for node in &backbone {
            relay_order.push(*node);
        }

        let mut supernode_order = vec![root];
        supernode_order.extend(backbone.iter().copied());
        for supernode in supernode_order {
            if let Some(children) = anchors.get_mut(&supernode) {
                let super_pos = self.peer_pos(supernode).unwrap_or(Vec2::ZERO);
                children.sort_by(|a, b| {
                    let pa = self.peer_pos(*a).unwrap_or(super_pos);
                    let pb = self.peer_pos(*b).unwrap_or(super_pos);
                    let da = {
                        let d = pa - super_pos;
                        d.x * d.x + d.y * d.y
                    };
                    let db = {
                        let d = pb - super_pos;
                        d.x * d.x + d.y * d.y
                    };
                    da.partial_cmp(&db)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then_with(|| a.cmp(b))
                });
                relay_order.extend(children.iter().copied());
            }
        }
        relay_order
    }

    fn build_relay_order(
        all_nodes: &[matchbox_socket::PeerId],
        supernodes: &[matchbox_socket::PeerId],
    ) -> Vec<matchbox_socket::PeerId> {
        let super_set: HashSet<matchbox_socket::PeerId> = supernodes.iter().copied().collect();
        let mut relay_order = supernodes.to_vec();
        for node in all_nodes {
            if !super_set.contains(node) {
                relay_order.push(*node);
            }
        }
        relay_order
    }

    fn tree_assignment_for_index(
        relay_order: &[matchbox_socket::PeerId],
        local_idx: usize,
        fanout: usize,
    ) -> (
        Option<matchbox_socket::PeerId>,
        Option<matchbox_socket::PeerId>,
        Vec<matchbox_socket::PeerId>,
    ) {
        let fanout = fanout.clamp(Self::MIN_FANOUT, Self::MAX_FANOUT);
        let parent = if local_idx == 0 {
            None
        } else {
            let parent_idx = (local_idx - 1) / fanout;
            relay_order.get(parent_idx).copied()
        };

        let backup_parent = if local_idx <= 1 {
            None
        } else {
            let alt_idx = (local_idx - 2) / fanout;
            relay_order.get(alt_idx).copied()
        };

        let mut children = Vec::new();
        for child_slot in 0..fanout {
            let child_idx = local_idx * fanout + child_slot + 1;
            if let Some(child) = relay_order.get(child_idx) {
                children.push(*child);
            }
        }

        (parent, backup_parent, children)
    }
    pub fn new() -> Self {
        Self {
            socket: None,
            socket_closed: Rc::new(Cell::new(false)),
            state: NetworkState::Disconnected,
            room_code: String::new(),
            local_player_name: Self::generate_default_name(),
            remote_players: HashMap::new(),
            local_peer_id: None,
            local_peer_hash: None,
            supernode_id: None,
            super_root_id: None,
            supernode_set: Vec::new(),
            peer_anchor_supernode: HashMap::new(),
            relay_parent: None,
            relay_backup_parent: None,
            relay_active_parent: None,
            relay_fanout: Self::MIN_FANOUT,
            relay_children: Vec::new(),
            desired_peer_set: HashSet::new(),
            discovery_attached: false,
            discovery_attach_frame: None,
            bootstrap_full_mesh_active: false,
            frames_without_peer_connection: 0,
            relay_epoch: 0,
            last_parent_switch_frame: 0,
            stale_parent_events: 0,
            last_peer_message_frames: HashMap::new(),
            relay_telemetry: RelayTelemetry::default(),
            last_update_frame: 0,
            last_lowpri_chat_frame: u32::MAX,
            last_lowpri_vote_frame: u32::MAX,
            last_lowpri_ack_frame: u32::MAX,
            last_telemetry_log_frame: 0,
            area_authorities: HashMap::new(),
            last_topology_broadcast_frame: 0,
            last_area_update_broadcast_frame: 0,
            local_last_pos: None,
            local_stats: PlayerStats::default(),
            remote_stats: HashMap::new(),
            pending_player_names: HashMap::new(),
            peer_id_lookup: HashMap::new(),
            peer_hash_lookup: HashMap::new(),
            pending_messages: Vec::new(),
            is_host: false,
            pending_enemy_sync: None,
            pending_enemy_damage: Vec::new(),
            pending_wave_start: None,
            pending_enemy_kills_optimistic: Vec::new(),
            pending_enemy_kills_confirmed: Vec::new(),
            pending_player_deaths_optimistic: Vec::new(),
            pending_player_deaths_confirmed: Vec::new(),
            enemy_kill_confirmations: HashMap::new(),
            player_death_confirmations: HashMap::new(),
            applied_event_ids: HashMap::new(),
            optimistic_enemy_event_ids: HashMap::new(),
            optimistic_death_event_ids: HashMap::new(),
            pending_paid_obstacles: Vec::new(),
            pending_paid_obstacle_acks: Vec::new(),
            pending_paid_abilities: Vec::new(),
            pending_paid_ability_acks: Vec::new(),
            pending_cannon_shots: Vec::new(),
            pending_chat_messages: Vec::new(),
            pending_vote_mutes: Vec::new(),
            muted_hashes: HashSet::new(),
            vote_mutes: HashMap::new(),
            pending_input_frames: Vec::new(),
            relay_player_states: HashMap::new(),
            relay_input_frames: Vec::new(),
            downlink_player_batches: Vec::new(),
            downlink_input_batches: Vec::new(),
            latency_ms: HashMap::new(),
            latency_samples: HashMap::new(),
            supernode_scores: HashMap::new(),
            bad_supernodes: HashSet::new(),
            last_enemy_sync_frame: 0,
            paid_obstacle_confirmations: HashMap::new(),
            paid_ability_confirmations: HashMap::new(),
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
    pub fn create_room(&mut self, signaling_server: &str, ice_config: &IceConfig) -> String {
        self.room_code = Self::generate_room_code();
        self.is_host = true; // Room creator is the host
        let room_code = self.room_code.clone();
        self.connect(signaling_server, &room_code, ice_config);
        self.room_code.clone()
    }

    /// Join an existing room by code
    pub fn join_room(&mut self, signaling_server: &str, room_code: &str, ice_config: &IceConfig) {
        self.room_code = room_code.to_uppercase();
        self.is_host = false; // Joiners are not hosts
        let room_code = self.room_code.clone();
        self.connect(signaling_server, &room_code, ice_config);
    }

    fn connect(&mut self, signaling_server: &str, room_code: &str, ice_config: &IceConfig) {
        // Use game-specific room prefix to avoid conflicts with other matchbox games
        // ?next=2 tells matchbox to start handshake when 2 peers connect
        let room_url = format!("{}/slime_armies_{}", signaling_server, room_code);

        web_sys::console::log_1(&format!("Connecting to room: {}", room_url).into());

        self.state = NetworkState::Connecting;

        // Reset the closed flag for new connection
        self.socket_closed = Rc::new(Cell::new(false));
        let closed_flag = Rc::clone(&self.socket_closed);

        let ice_server = matchbox_socket::RtcIceServerConfig {
            urls: ice_config.urls.clone(),
            username: ice_config.username.clone(),
            credential: ice_config.credential.clone(),
        };
        let (mut socket, loop_fut) = WebRtcSocket::builder(&room_url)
            .ice_server(ice_server)
            .add_reliable_channel()
            .build();
        // Start in sparse-link mode immediately to avoid initial full-mesh handshake bursts.
        socket.set_desired_peers(Vec::new());
        self.socket = Some(socket);
        self.desired_peer_set.clear();
        self.discovery_attached = true;
        self.discovery_attach_frame = None;
        self.bootstrap_full_mesh_active = false;
        self.frames_without_peer_connection = 0;
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
        self.local_peer_hash = None;
        self.supernode_id = None;
        self.super_root_id = None;
        self.supernode_set.clear();
        self.peer_anchor_supernode.clear();
        self.relay_parent = None;
        self.relay_backup_parent = None;
        self.relay_active_parent = None;
        self.relay_fanout = Self::MIN_FANOUT;
        self.relay_children.clear();
        self.desired_peer_set.clear();
        self.discovery_attached = false;
        self.discovery_attach_frame = None;
        self.bootstrap_full_mesh_active = false;
        self.frames_without_peer_connection = 0;
        self.relay_epoch = 0;
        self.last_parent_switch_frame = 0;
        self.stale_parent_events = 0;
        self.last_peer_message_frames.clear();
        self.relay_telemetry = RelayTelemetry::default();
        self.last_update_frame = 0;
        self.last_lowpri_chat_frame = u32::MAX;
        self.last_lowpri_vote_frame = u32::MAX;
        self.last_lowpri_ack_frame = u32::MAX;
        self.last_telemetry_log_frame = 0;
        self.area_authorities.clear();
        self.local_last_pos = None;
        self.pending_player_names.clear();
        self.peer_id_lookup.clear();
        self.peer_hash_lookup.clear();
        self.bad_supernodes.clear();
        self.last_enemy_sync_frame = 0;
        self.paid_obstacle_confirmations.clear();
        self.paid_ability_confirmations.clear();
        self.latency_ms.clear();
        self.latency_samples.clear();
        self.supernode_scores.clear();
        self.pending_chat_messages.clear();
        self.pending_vote_mutes.clear();
        self.muted_hashes.clear();
        self.vote_mutes.clear();
        self.relay_player_states.clear();
        self.relay_input_frames.clear();
        self.downlink_player_batches.clear();
        self.downlink_input_batches.clear();
        self.enemy_kill_confirmations.clear();
        self.player_death_confirmations.clear();
        self.applied_event_ids.clear();
        self.optimistic_enemy_event_ids.clear();
        self.optimistic_death_event_ids.clear();
    }

    /// Poll for network events and update state
    /// Returns true if update succeeded, false if connection failed
    pub fn update(&mut self, current_frame: u32) -> bool {
        self.last_update_frame = current_frame;
        // Check if socket loop has ended (connection failed)
        if self.socket_closed.get() {
            web_sys::console::log_1(&"Network connection closed".into());
            self.socket = None;
            self.state = NetworkState::Error("Connection failed".to_string());
            return false;
        }

        let (local_id, connected_peers, known_peers) = {
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
                    let hash = Self::hash_peer_id(&peer_id_str);
                    self.peer_hash_lookup.insert(hash, peer_id_str.clone());
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
                    if let Some(peer_id) = self.peer_id_lookup.get(&peer_id_str).copied() {
                        self.bad_supernodes.remove(&peer_id);
                        self.supernode_scores.remove(&peer_id);
                        self.latency_ms.remove(&peer_id);
                        self.latency_samples.remove(&peer_id);
                        self.last_peer_message_frames.remove(&peer_id);
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
                let hash = Self::hash_peer_id(&peer_id_str);
                self.peer_hash_lookup.insert(hash, peer_id_str.clone());
                self.last_peer_message_frames.insert(peer_id, current_frame);
                self.relay_telemetry.recv_messages = self.relay_telemetry.recv_messages.saturating_add(1);
                if let Some(msg) = NetMessage::from_bytes(&data) {
                    self.pending_messages.push((peer_id_str, msg));
                } else {
                    self.relay_telemetry.dropped_messages = self.relay_telemetry.dropped_messages.saturating_add(1);
                }
            }

            let local_id = socket.id();
            if let Some(local_id) = local_id {
                let local_id_str = format!("{:?}", local_id);
                let hash = Self::hash_peer_id(&local_id_str);
                self.local_peer_hash = Some(hash);
                self.peer_hash_lookup.insert(hash, local_id_str);
            }
            let connected_peers: Vec<_> = socket.connected_peers().collect();
            let mut known_peers: Vec<_> = if self.discovery_attached {
                socket.known_peers().collect()
            } else {
                connected_peers.clone()
            };
            known_peers.extend(connected_peers.iter().copied());
            known_peers.sort();
            known_peers.dedup();
            (local_id, connected_peers, known_peers)
        };

        if self.discovery_attached && self.discovery_attach_frame.is_none() && local_id.is_some() {
            self.discovery_attach_frame = Some(current_frame);
        }
        if connected_peers.is_empty() {
            self.frames_without_peer_connection =
                self.frames_without_peer_connection.saturating_add(1);
        } else {
            self.frames_without_peer_connection = 0;
        }

        let local_peer_str = local_id.map(|id| format!("{:?}", id));
        let known_peer_strs: HashSet<String> = known_peers
            .iter()
            .map(|id| format!("{:?}", id))
            .collect();
        self.peer_id_lookup.retain(|peer_id, _| {
            known_peer_strs.contains(peer_id)
                || local_peer_str
                    .as_ref()
                    .map(|local| local == peer_id)
                    .unwrap_or(false)
        });
        self.peer_hash_lookup.retain(|_, peer_id| {
            known_peer_strs.contains(peer_id)
                || local_peer_str
                    .as_ref()
                    .map(|local| local == peer_id)
                    .unwrap_or(false)
        });

        // Process pending messages
        let mut ping_replies: Vec<(PeerId, Ping)> = Vec::new();
        let mut pong_updates: Vec<(PeerId, Pong)> = Vec::new();
        let mut score_updates: Vec<(PeerId, SupernodeScore)> = Vec::new();
        let mut enemy_syncs: Vec<(PeerId, EnemySync)> = Vec::new();
        let mut enemy_damages: Vec<(PeerId, EnemyDamage)> = Vec::new();
        let mut wave_starts: Vec<(PeerId, WaveStart)> = Vec::new();
        let mut enemy_kills: Vec<(PeerId, EnemyKill)> = Vec::new();
        let mut player_deaths: Vec<(PeerId, PlayerDeath)> = Vec::new();
        let mut paid_obstacles: Vec<(PeerId, PaidObstacle)> = Vec::new();
        let mut paid_abilities: Vec<(PeerId, PaidAbility)> = Vec::new();
        let mut paid_obstacle_acks: Vec<(PeerId, PaidObstacleAck)> = Vec::new();
        let mut paid_ability_acks: Vec<(PeerId, PaidAbilityAck)> = Vec::new();
        let mut cannon_shots: Vec<(PeerId, CannonShot)> = Vec::new();
        let mut chat_messages: Vec<(PeerId, ChatMessage)> = Vec::new();
        let mut vote_mutes: Vec<(PeerId, VoteMute)> = Vec::new();
        let mut player_updates: Vec<(PeerId, PlayerState)> = Vec::new();
        let mut input_frames: Vec<(PeerId, InputFrame)> = Vec::new();
        let mut player_batches: Vec<(PeerId, PlayerStateBatch)> = Vec::new();
        let mut input_batches: Vec<(PeerId, InputFrameBatch)> = Vec::new();
        let mut topology_updates: Vec<(PeerId, TopologyUpdate)> = Vec::new();
        let mut area_authority_updates: Vec<(PeerId, AreaAuthorityUpdate)> = Vec::new();

        for (peer_id, msg) in self.pending_messages.drain(..) {
            match msg {
                NetMessage::PlayerUpdate(state) => {
                    player_updates.push((peer_id, state));
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
                    enemy_damages.push((peer_id, damage));
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
                NetMessage::PaidAbilityEvent(ability) => {
                    paid_abilities.push((peer_id, ability));
                }
                NetMessage::PaidObstacleSyncEvent(sync) => {
                    for obstacle in sync.obstacles {
                        self.pending_paid_obstacles.push(("sync".to_string(), obstacle));
                    }
                }
                NetMessage::PaidObstacleAckEvent(ack) => {
                    paid_obstacle_acks.push((peer_id, ack));
                }
                NetMessage::PaidAbilityAckEvent(ack) => {
                    paid_ability_acks.push((peer_id, ack));
                }
                NetMessage::CannonShotEvent(shot) => {
                    cannon_shots.push((peer_id, shot));
                }
                NetMessage::ChatMessageEvent(chat) => {
                    chat_messages.push((peer_id, chat));
                }
                NetMessage::VoteMuteEvent(vote) => {
                    vote_mutes.push((peer_id, vote));
                }
                NetMessage::InputFrameEvent(frame) => {
                    input_frames.push((peer_id, frame));
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
                NetMessage::PlayerStateBatchEvent(batch) => {
                    player_batches.push((peer_id, batch));
                }
                NetMessage::InputFrameBatchEvent(batch) => {
                    input_batches.push((peer_id, batch));
                }
                NetMessage::TopologyUpdateEvent(update) => {
                    topology_updates.push((peer_id, update));
                }
                NetMessage::AreaAuthorityUpdateEvent(update) => {
                    area_authority_updates.push((peer_id, update));
                }
            }
        }

        for (peer_id, update) in topology_updates {
            let from_parent = self.is_parent_sender(&peer_id);
            let from_root = self.is_authoritative_sender(&peer_id);
            if (from_parent || from_root) && !self.relay_children.is_empty() {
                self.send_downstream_or_broadcast(
                    NetMessage::TopologyUpdateEvent(update.clone()).to_bytes(),
                    self.resolve_peer_id(&peer_id),
                );
            }
            self.apply_topology_update(&peer_id, update, current_frame);
        }
        for (peer_id, update) in area_authority_updates {
            let from_parent = self.is_parent_sender(&peer_id);
            let from_root = self.is_authoritative_sender(&peer_id);
            if (from_parent || from_root) && !self.relay_children.is_empty() {
                self.send_downstream_or_broadcast(
                    NetMessage::AreaAuthorityUpdateEvent(update.clone()).to_bytes(),
                    self.resolve_peer_id(&peer_id),
                );
            }
            self.apply_area_authority_update(&peer_id, update);
        }

        for (peer_id, state) in player_updates {
            if !self.is_parent_sender(&peer_id) {
                let peer_hash = Self::hash_peer_id(&peer_id);
                self.queue_relay_player_state(peer_hash, state);
            }
            if self.is_host || self.is_authoritative_sender(&peer_id) {
                if let Some(remote) = self.remote_players.get_mut(&peer_id) {
                    remote.update_state(&state, current_frame);
                } else {
                    let name = self.pending_player_names.remove(&peer_id).unwrap_or_else(|| "Player".to_string());
                    self.remote_players.insert(peer_id.clone(), RemotePlayer::new(name, &state, current_frame));
                }
            }
        }

        for (peer_id, frame) in input_frames {
            if !self.is_parent_sender(&peer_id) {
                let peer_hash = Self::hash_peer_id(&peer_id);
                self.queue_relay_input_frame(peer_hash, frame);
            }
            self.pending_input_frames.push((peer_id, frame));
        }

        for (peer_id, batch) in player_batches {
            let from_parent = self.is_parent_sender(&peer_id);
            let from_child = self.is_child_sender(&peer_id);
            if !(from_parent || from_child || self.is_host || self.is_authoritative_sender(&peer_id)) {
                continue;
            }
            for entry in &batch.entries {
                if let Some(peer_id) = self.resolve_peer_hash(entry.peer_hash) {
                    if let Some(remote) = self.remote_players.get_mut(&peer_id) {
                        remote.update_state(&entry.state, current_frame);
                    } else {
                        let name = self.pending_player_names.remove(&peer_id).unwrap_or_else(|| "Player".to_string());
                        self.remote_players.insert(peer_id.clone(), RemotePlayer::new(name, &entry.state, current_frame));
                    }
                }
            }
            if from_parent && !self.relay_children.is_empty() {
                self.queue_downlink_player_batch(batch);
            } else if from_child {
                if self.relay_active_parent.or(self.relay_parent).is_some() {
                    for entry in batch.entries {
                        self.queue_relay_player_state(entry.peer_hash, entry.state);
                    }
                } else if !self.relay_children.is_empty() {
                    self.queue_downlink_player_batch(batch);
                }
            }
        }

        for (peer_id, batch) in input_batches {
            let from_parent = self.is_parent_sender(&peer_id);
            let from_child = self.is_child_sender(&peer_id);
            if !(from_parent || from_child || self.is_host || self.is_authoritative_sender(&peer_id)) {
                continue;
            }
            for entry in &batch.entries {
                if let Some(peer_id) = self.resolve_peer_hash(entry.peer_hash) {
                    self.pending_input_frames.push((peer_id, entry.frame));
                }
            }
            if from_parent && !self.relay_children.is_empty() {
                self.queue_downlink_input_batch(batch);
            } else if from_child {
                if self.relay_active_parent.or(self.relay_parent).is_some() {
                    for entry in batch.entries {
                        self.queue_relay_input_frame(entry.peer_hash, entry.frame);
                    }
                } else if !self.relay_children.is_empty() {
                    self.queue_downlink_input_batch(batch);
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
        for (peer_id, damage) in enemy_damages {
            if self.is_host {
                self.pending_enemy_damage.push(damage);
            } else if !self.is_parent_sender(&peer_id) {
                self.send_upstream_or_broadcast(NetMessage::EnemyDamageEvent(damage).to_bytes());
            }
        }
        for (peer_id, sync) in enemy_syncs {
            if !self.is_host && (self.is_authoritative_sender(&peer_id) || self.is_parent_sender(&peer_id)) {
                self.pending_enemy_sync = Some(sync.clone());
                self.forward_enemy_sync_down(sync);
            } else if !self.is_host {
                self.relay_telemetry.dropped_messages = self.relay_telemetry.dropped_messages.saturating_add(1);
                web_sys::console::log_1(&format!("Dropped enemy sync from non-authoritative {}", peer_id).into());
            }
        }
        for (peer_id, wave_start) in wave_starts {
            if !self.is_host && (self.is_authoritative_sender(&peer_id) || self.is_parent_sender(&peer_id)) {
                self.pending_wave_start = Some(wave_start);
                self.forward_wave_start_down(wave_start);
            } else if !self.is_host {
                self.relay_telemetry.dropped_messages = self.relay_telemetry.dropped_messages.saturating_add(1);
                web_sys::console::log_1(&format!("Dropped wave start from non-authoritative {}", peer_id).into());
            }
        }
        for (peer_id, kill) in enemy_kills {
            self.handle_enemy_kill(peer_id, kill, current_frame);
        }
        for (peer_id, death) in player_deaths {
            self.handle_player_death(peer_id, death, current_frame);
        }
        for (peer_id, obstacle) in paid_obstacles {
            let from_parent = self.is_parent_sender(&peer_id);
            let from_root = self.is_authoritative_sender(&peer_id);
            let from_child = self.is_child_sender(&peer_id);
            if self.is_host {
                self.pending_paid_obstacles.push((peer_id.clone(), obstacle));
                let exclude = self.resolve_peer_id(&peer_id);
                self.relay_paid_obstacle(obstacle, exclude);
            } else if from_root || from_parent {
                let exclude = self.resolve_peer_id(&peer_id);
                self.pending_paid_obstacles.push((peer_id, obstacle));
                if from_parent && !self.relay_children.is_empty() {
                    self.relay_paid_obstacle(obstacle, exclude);
                }
            } else if from_child {
                self.send_upstream_or_broadcast(NetMessage::PaidObstacleEvent(obstacle).to_bytes());
            }
        }
        for (peer_id, ability) in paid_abilities {
            let from_parent = self.is_parent_sender(&peer_id);
            let from_root = self.is_authoritative_sender(&peer_id);
            let from_child = self.is_child_sender(&peer_id);
            if self.is_host {
                self.pending_paid_abilities.push((peer_id.clone(), ability));
                let exclude = self.resolve_peer_id(&peer_id);
                self.relay_paid_ability(ability, exclude);
            } else if from_root || from_parent {
                let exclude = self.resolve_peer_id(&peer_id);
                self.pending_paid_abilities.push((peer_id, ability));
                if from_parent && !self.relay_children.is_empty() {
                    self.relay_paid_ability(ability, exclude);
                }
            } else if from_child {
                self.send_upstream_or_broadcast(NetMessage::PaidAbilityEvent(ability).to_bytes());
            }
        }
        for (peer_id, shot) in cannon_shots {
            let from_parent = self.is_parent_sender(&peer_id);
            let from_root = self.is_authoritative_sender(&peer_id);
            let from_child = self.is_child_sender(&peer_id);
            if from_root || from_parent {
                self.pending_cannon_shots.push(shot);
                if from_parent && !self.relay_children.is_empty() {
                    self.send_downstream_or_broadcast(
                        NetMessage::CannonShotEvent(shot).to_bytes(),
                        self.resolve_peer_id(&peer_id),
                    );
                }
            } else if from_child {
                self.send_upstream_or_broadcast(NetMessage::CannonShotEvent(shot).to_bytes());
            }
        }
        for (peer_id, ack) in paid_obstacle_acks {
            let from_parent = self.is_parent_sender(&peer_id);
            let from_root = self.is_authoritative_sender(&peer_id);
            let from_child = self.is_child_sender(&peer_id);
            self.pending_paid_obstacle_acks.push((peer_id.clone(), ack));
            if self.is_host {
                self.send_low_priority_downstream_or_broadcast(
                    LowPriorityTopic::Ack,
                    NetMessage::PaidObstacleAckEvent(ack).to_bytes(),
                    self.resolve_peer_id(&peer_id),
                );
            } else if from_parent {
                if !self.relay_children.is_empty() {
                    self.send_low_priority_downstream_or_broadcast(
                        LowPriorityTopic::Ack,
                        NetMessage::PaidObstacleAckEvent(ack).to_bytes(),
                        self.resolve_peer_id(&peer_id),
                    );
                }
            } else if from_child {
                self.send_low_priority_upstream_or_broadcast(
                    LowPriorityTopic::Ack,
                    NetMessage::PaidObstacleAckEvent(ack).to_bytes(),
                );
            } else if !from_root {
                // Ignore unknown senders after recording; forwarded copies still need tree path.
                self.send_low_priority_upstream_or_broadcast(
                    LowPriorityTopic::Ack,
                    NetMessage::PaidObstacleAckEvent(ack).to_bytes(),
                );
            }
        }
        for (peer_id, ack) in paid_ability_acks {
            let from_parent = self.is_parent_sender(&peer_id);
            let from_root = self.is_authoritative_sender(&peer_id);
            let from_child = self.is_child_sender(&peer_id);
            self.pending_paid_ability_acks.push((peer_id.clone(), ack));
            if self.is_host {
                self.send_low_priority_downstream_or_broadcast(
                    LowPriorityTopic::Ack,
                    NetMessage::PaidAbilityAckEvent(ack).to_bytes(),
                    self.resolve_peer_id(&peer_id),
                );
            } else if from_parent {
                if !self.relay_children.is_empty() {
                    self.send_low_priority_downstream_or_broadcast(
                        LowPriorityTopic::Ack,
                        NetMessage::PaidAbilityAckEvent(ack).to_bytes(),
                        self.resolve_peer_id(&peer_id),
                    );
                }
            } else if from_child {
                self.send_low_priority_upstream_or_broadcast(
                    LowPriorityTopic::Ack,
                    NetMessage::PaidAbilityAckEvent(ack).to_bytes(),
                );
            } else if !from_root {
                self.send_low_priority_upstream_or_broadcast(
                    LowPriorityTopic::Ack,
                    NetMessage::PaidAbilityAckEvent(ack).to_bytes(),
                );
            }
        }
        for (_, chat) in &chat_messages {
            if !self.is_muted(chat.sender_hash) {
                self.pending_chat_messages.push(chat.clone());
            }
        }
        for (_, vote) in &vote_mutes {
            if self.register_vote_mute(*vote) {
                self.pending_vote_mutes.push(*vote);
            }
        }
        for (peer_id, chat) in chat_messages {
            self.relay_low_priority_control_message(
                &peer_id,
                LowPriorityTopic::Chat,
                NetMessage::ChatMessageEvent(chat).to_bytes(),
            );
        }
        for (peer_id, vote) in vote_mutes {
            self.relay_low_priority_control_message(
                &peer_id,
                LowPriorityTopic::Vote,
                NetMessage::VoteMuteEvent(vote).to_bytes(),
            );
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
        let prev_relay_epoch = self.relay_epoch;
        let allow_local_topology_recompute = self.discovery_attached || self.is_host;
        if allow_local_topology_recompute {
            self.update_supernode_from(local_id, &connected_peers, &known_peers, current_frame);
        } else if let Some(id) = local_id {
            self.local_peer_id = Some(id);
        }
        self.maybe_failover_parent(current_frame);
        self.maybe_manage_bootstrap_full_mesh(&known_peers, &connected_peers);
        self.update_desired_peers(&known_peers, &connected_peers);
        self.maybe_detach_discovery(current_frame, &connected_peers);
        if prev_supernode != self.supernode_id {
            web_sys::console::log_1(&format!("Supernode updated: {:?} -> {:?}", prev_supernode, self.supernode_id).into());
        }
        if self.is_host && self.relay_epoch != prev_relay_epoch {
            self.last_topology_broadcast_frame = current_frame;
            self.last_area_update_broadcast_frame = current_frame;
            self.broadcast_topology_update();
            self.broadcast_area_authorities();
        }
        if self.is_host && current_frame.saturating_sub(self.last_topology_broadcast_frame) >= 120 {
            self.last_topology_broadcast_frame = current_frame;
            self.broadcast_topology_update();
        }

        if !connected_peers.is_empty() {
            self.state = NetworkState::Connected;
        } else if self.remote_players.is_empty() {
            self.state = NetworkState::WaitingForPeers;
        }
        self.prune_event_confirmations(current_frame);
        self.tick_latency(current_frame, &connected_peers);
        self.log_telemetry_periodic(current_frame);

        true
    }

    fn maybe_manage_bootstrap_full_mesh(
        &mut self,
        known_peers: &[matchbox_socket::PeerId],
        connected_peers: &[matchbox_socket::PeerId],
    ) {
        if !self.discovery_attached {
            self.bootstrap_full_mesh_active = false;
            return;
        }

        if self.bootstrap_full_mesh_active {
            if connected_peers.is_empty() {
                return;
            }
            self.bootstrap_full_mesh_active = false;
            self.desired_peer_set.clear();
            self.update_desired_peers(known_peers, connected_peers);
            web_sys::console::log_1(&"Bootstrap probe mode disabled after first peer connection".into());
            return;
        }

        if !connected_peers.is_empty() {
            return;
        }
        if self.frames_without_peer_connection < Self::BOOTSTRAP_FULLMESH_TRIGGER_FRAMES {
            return;
        }
        if known_peers.is_empty() {
            return;
        }

        let mut probe = known_peers.to_vec();
        probe.sort();
        if let Some(local) = self.local_peer_id {
            probe.retain(|id| *id != local);
        }
        if probe.is_empty() {
            return;
        }
        let seed = self
            .local_peer_id
            .map(|id| Self::hash_peer_id(&format!("{:?}", id)) as usize)
            .unwrap_or(0);
        let rotate = (self.relay_epoch as usize).wrapping_add(seed) % probe.len();
        probe.rotate_left(rotate);
        let target = Self::BOOTSTRAP_PROBE_LINKS
            .min(self.role_link_budget().max(2))
            .min(probe.len());
        let probe_set: HashSet<matchbox_socket::PeerId> = probe.into_iter().take(target).collect();

        if let Some(socket) = &mut self.socket {
            socket.set_desired_peers(probe_set.iter().copied());
            self.bootstrap_full_mesh_active = true;
            self.desired_peer_set = probe_set;
            web_sys::console::log_1(
                &format!(
                    "Bootstrap probe mode enabled (known peers: {}, no links for {} frames)",
                    known_peers.len().saturating_sub(1),
                    self.frames_without_peer_connection
                )
                .into(),
            );
        }
    }

    fn maybe_detach_discovery(
        &mut self,
        current_frame: u32,
        connected_peers: &[matchbox_socket::PeerId],
    ) {
        if !self.discovery_attached {
            return;
        }
        if self.is_host || self.local_is_supernode() {
            return;
        }
        if self.relay_epoch == 0 {
            return;
        }
        let attach_frame = self.discovery_attach_frame.unwrap_or(current_frame);
        if current_frame.saturating_sub(attach_frame) < Self::DISCOVERY_MIN_ATTACH_FRAMES {
            return;
        }
        if connected_peers.is_empty() {
            return;
        }
        let has_overlay_route = self
            .relay_active_parent
            .or(self.relay_parent)
            .is_some()
            || !self.relay_children.is_empty()
            || self.desired_peer_set.len() >= 2;
        if !has_overlay_route {
            return;
        }

        if let Some(socket) = &mut self.socket {
            socket.detach_signaling();
            self.discovery_attached = false;
            self.bootstrap_full_mesh_active = false;
            web_sys::console::log_1(&"Discovery detached: using gameplay overlay links only".into());
        }
    }

    fn update_supernode_from(
        &mut self,
        local_id: Option<matchbox_socket::PeerId>,
        connected_peers: &[matchbox_socket::PeerId],
        known_peers: &[matchbox_socket::PeerId],
        current_frame: u32,
    ) {
        if let Some(id) = local_id {
            self.local_peer_id = Some(id);
        }

        let local_id = match self.local_peer_id {
            Some(id) => id,
            None => return,
        };

        self.supernode_scores.retain(|peer_id, _| connected_peers.contains(peer_id));
        self.latency_ms.retain(|peer_id, _| connected_peers.contains(peer_id));
        self.latency_samples.retain(|peer_id, _| connected_peers.contains(peer_id));

        let mut all_nodes = known_peers.to_vec();
        all_nodes.push(local_id);
        all_nodes.sort();
        all_nodes.dedup();
        if all_nodes.is_empty() {
            return;
        }

        let mut active_areas: HashSet<u32> = HashSet::new();
        for node in &all_nodes {
            let area = self.peer_pos(*node).map(Self::area_id_from_pos).unwrap_or(0);
            active_areas.insert(area);
        }
        let target_k = Self::choose_dynamic_supernode_count(all_nodes.len(), active_areas.len());
        let supernodes = self.select_supernodes_dynamic(&all_nodes, target_k);
        if supernodes.is_empty() {
            return;
        }
        let super_root = if let Some(prev_root) = self.super_root_id {
            if supernodes.contains(&prev_root) {
                prev_root
            } else {
                supernodes[0]
            }
        } else {
            supernodes[0]
        };

        let relay_order = self.build_clustered_relay_order(&all_nodes, &supernodes, super_root);
        let relay_fanout = Self::choose_dynamic_fanout(all_nodes.len());

        let mut index_by_id: HashMap<matchbox_socket::PeerId, usize> = HashMap::new();
        for (idx, id) in relay_order.iter().enumerate() {
            index_by_id.insert(*id, idx);
        }

        let local_idx = match index_by_id.get(&local_id) {
            Some(idx) => *idx,
            None => 0,
        };

        let (parent, backup_parent_raw, children) =
            Self::tree_assignment_for_index(&relay_order, local_idx, relay_fanout);
        let backup_parent = backup_parent_raw.filter(|id| Some(*id) != parent);

        let topology_changed = self.super_root_id != Some(super_root)
            || self.relay_parent != parent
            || self.relay_backup_parent != backup_parent
            || self.relay_children != children
            || self.relay_fanout != relay_fanout
            || self.supernode_set != supernodes;
        let parent_changed = self.relay_parent != parent || self.relay_backup_parent != backup_parent;

        self.supernode_set = supernodes;
        self.super_root_id = Some(super_root);
        // Keep existing callsites working: supernode_id remains the authoritative root.
        self.supernode_id = Some(super_root);
        self.is_host = super_root == local_id;
        let prev_parent = self.relay_parent;
        let prev_active_parent = self.relay_active_parent;
        self.relay_parent = parent;
        self.relay_backup_parent = backup_parent;
        self.relay_fanout = relay_fanout;
        if parent_changed && prev_parent.is_some() && prev_parent != self.relay_parent {
            self.relay_backup_parent = prev_parent;
        }
        self.relay_children = children;
        if topology_changed {
            self.relay_epoch = self.relay_epoch.wrapping_add(1);
        }
        if self.is_host {
            self.relay_active_parent = None;
        } else if self.relay_active_parent.is_none() {
            self.relay_active_parent = self.relay_parent;
        } else if let Some(active) = self.relay_active_parent {
            let valid = Some(active) == self.relay_parent || Some(active) == self.relay_backup_parent;
            if !valid {
                self.relay_active_parent = self.relay_parent;
            }
        }
        if prev_active_parent != self.relay_active_parent {
            self.last_parent_switch_frame = current_frame;
        }

        self.recompute_area_authorities(current_frame);
    }

    fn recompute_area_authorities(&mut self, current_frame: u32) {
        if self.supernode_set.is_empty() {
            self.area_authorities.clear();
            return;
        }

        let mut area_samples: HashMap<u32, (Vec2, u32)> = HashMap::new();
        if let Some(pos) = self.local_last_pos {
            let area_id = Self::area_id_from_pos(pos);
            area_samples.insert(area_id, (pos, 1));
        }
        for remote in self.remote_players.values() {
            let area_id = Self::area_id_from_pos(remote.pos);
            if let Some((sum, count)) = area_samples.get_mut(&area_id) {
                *sum += remote.pos;
                *count = count.saturating_add(1);
            } else {
                area_samples.insert(area_id, (remote.pos, 1));
            }
        }
        if area_samples.is_empty() {
            return;
        }

        let mut supernode_positions: HashMap<u64, Vec2> = HashMap::new();
        for supernode in &self.supernode_set {
            let hash = Self::hash_peer_id(&format!("{:?}", supernode));
            if Some(*supernode) == self.local_peer_id {
                if let Some(pos) = self.local_last_pos {
                    supernode_positions.insert(hash, pos);
                }
            } else if let Some(peer_id) = self.peer_hash_lookup.get(&hash) {
                if let Some(remote) = self.remote_players.get(peer_id) {
                    supernode_positions.insert(hash, remote.pos);
                }
            }
        }

        let mut authorities = HashMap::new();
        for (area_id, (sum, count)) in area_samples {
            let center = if count > 0 {
                sum / count as f32
            } else {
                sum
            };
            let mut best: Option<(u64, f32)> = None;
            for supernode in &self.supernode_set {
                let hash = Self::hash_peer_id(&format!("{:?}", supernode));
                let dist_score = if let Some(pos) = supernode_positions.get(&hash) {
                    let d = *pos - center;
                    d.x * d.x + d.y * d.y
                } else {
                    1_000_000_000.0
                };
                let latency = self.latency_ms.get(supernode).copied().unwrap_or(1000) as f32;
                let score = dist_score + latency * 250.0 + (hash as u32 ^ area_id) as f32 * 0.0001;
                match best {
                    Some((_, best_score)) if score >= best_score => {}
                    _ => best = Some((hash, score)),
                }
            }
            if let Some((hash, _)) = best {
                authorities.insert(area_id, hash);
            }
        }

        self.area_authorities = authorities;
        if self.is_host && current_frame.saturating_sub(self.last_area_update_broadcast_frame) >= 120 {
            self.last_area_update_broadcast_frame = current_frame;
            self.broadcast_area_authorities();
        }
    }

    fn peer_seen_recently(&self, peer_id: matchbox_socket::PeerId, current_frame: u32) -> bool {
        let seen = self
            .last_peer_message_frames
            .get(&peer_id)
            .copied()
            .unwrap_or(self.last_parent_switch_frame);
        current_frame.saturating_sub(seen) <= Self::RELAY_PARENT_STALE_FRAMES
    }

    fn maybe_failover_parent(&mut self, current_frame: u32) {
        if self.is_host {
            self.relay_active_parent = None;
            return;
        }

        if self.relay_active_parent.is_none() {
            self.relay_active_parent = self.relay_parent;
            self.last_parent_switch_frame = current_frame;
            return;
        }

        if current_frame.saturating_sub(self.last_parent_switch_frame) < Self::RELAY_FAILOVER_COOLDOWN_FRAMES {
            return;
        }

        let primary = self.relay_parent;
        let backup = self.relay_backup_parent;
        let mut next_active = self.relay_active_parent;

        if let Some(parent) = primary {
            let parent_recent = self.peer_seen_recently(parent, current_frame);
            if !parent_recent {
                if let Some(backup_parent) = backup {
                    let backup_samples = self
                        .latency_samples
                        .get(&backup_parent)
                        .copied()
                        .unwrap_or(0);
                    let backup_recent = self.peer_seen_recently(backup_parent, current_frame);
                    if backup_recent || backup_samples >= Self::RELAY_FAILOVER_MIN_SAMPLES {
                        next_active = Some(backup_parent);
                    } else {
                        next_active = None;
                    }
                } else {
                    next_active = None;
                }
            } else if self.relay_active_parent != Some(parent) {
                next_active = Some(parent);
            }
        } else {
            next_active = None;
        }

        if next_active != self.relay_active_parent {
            self.relay_active_parent = next_active;
            self.last_parent_switch_frame = current_frame;
            self.stale_parent_events = self.stale_parent_events.saturating_add(1);
            self.relay_telemetry.stale_parent_switches = self.stale_parent_events;
            web_sys::console::log_1(
                &format!(
                    "Relay parent failover switch -> active {:?}, primary {:?}, backup {:?}",
                    self.relay_active_parent, self.relay_parent, self.relay_backup_parent
                )
                .into(),
            );
        }
    }

    fn local_is_supernode(&self) -> bool {
        if self.is_host {
            return true;
        }
        match self.local_peer_id {
            Some(id) => self.supernode_set.contains(&id),
            None => false,
        }
    }

    fn role_link_budget(&self) -> usize {
        if self.is_host {
            Self::ROOT_LINK_CAP
        } else if self.local_is_supernode() {
            Self::SUPERNODE_LINK_CAP
        } else {
            Self::LEAF_LINK_CAP
        }
    }

    fn desired_peer_links(
        &self,
        known_peers: &[matchbox_socket::PeerId],
        connected_peers: &[matchbox_socket::PeerId],
    ) -> HashSet<matchbox_socket::PeerId> {
        let mut desired = HashSet::new();
        let mut optional: Vec<matchbox_socket::PeerId> = Vec::new();
        let local_id = self.local_peer_id;
        let link_budget = self.role_link_budget();

        if self.is_host {
            desired.extend(self.relay_children.iter().copied());
        } else {
            if let Some(parent) = self.relay_parent {
                desired.insert(parent);
            }
            if let Some(backup) = self.relay_backup_parent {
                desired.insert(backup);
            }
            desired.extend(self.relay_children.iter().copied());
            if self.relay_parent.is_none() {
                if let Some(root) = self.super_root_id {
                    desired.insert(root);
                }
            }
        }

        // Keep one deterministic witness edge for 2-of-N confirmation paths.
        if let Some(local) = local_id {
            let mut sorted = known_peers.to_vec();
            sorted.sort();
            sorted.retain(|id| *id != local);
            if !sorted.is_empty() {
                let idx_seed = (self.relay_epoch as usize) % sorted.len();
                let witness = sorted[idx_seed];
                optional.push(witness);
            }
        }

        // Bootstrap: before topology converges, keep a small fixed-degree neighbor set.
        if desired.is_empty() {
            let mut sorted = known_peers.to_vec();
            sorted.sort();
            if let Some(local) = local_id {
                sorted.retain(|id| *id != local);
            }
            for peer in sorted.into_iter().take(4) {
                optional.push(peer);
            }
        }

        // Discovery admission window: supernodes/root keep a few spare candidate links
        // so newly joining peers can latch onto the gameplay overlay quickly.
        if self.discovery_attached && (self.is_host || self.local_is_supernode()) {
            let connected: HashSet<matchbox_socket::PeerId> = connected_peers.iter().copied().collect();
            let mut candidates: Vec<matchbox_socket::PeerId> = known_peers
                .iter()
                .copied()
                .filter(|id| Some(*id) != local_id)
                .filter(|id| !desired.contains(id))
                .collect();
            candidates.sort();
            candidates.sort_by_key(|id| connected.contains(id));
            if !candidates.is_empty() {
                let local_seed = local_id
                    .map(|id| Self::hash_peer_id(&format!("{:?}", id)) as usize)
                    .unwrap_or(0);
                let rotate = (self.relay_epoch as usize).wrapping_add(local_seed) % candidates.len();
                candidates.rotate_left(rotate);
                let admission_target = if self.is_host { 3 } else { 2 };
                for peer in candidates.into_iter().take(admission_target) {
                    optional.push(peer);
                }
            }
        }

        for peer in optional {
            if desired.contains(&peer) {
                continue;
            }
            if desired.len() >= link_budget {
                break;
            }
            desired.insert(peer);
        }

        if let Some(local) = local_id {
            desired.remove(&local);
        }
        desired
    }

    fn update_desired_peers(
        &mut self,
        known_peers: &[matchbox_socket::PeerId],
        connected_peers: &[matchbox_socket::PeerId],
    ) {
        if self.bootstrap_full_mesh_active {
            return;
        }
        let desired = self.desired_peer_links(known_peers, connected_peers);
        if desired == self.desired_peer_set {
            return;
        }
        if let Some(socket) = &mut self.socket {
            socket.set_desired_peers(desired.iter().copied());
        }
        self.desired_peer_set = desired;
    }

    fn apply_topology_update(&mut self, sender: &str, update: TopologyUpdate, current_frame: u32) {
        if self.is_host || self.relay_epoch > update.epoch {
            return;
        }
        if let Some(sender_id) = self.peer_id_lookup.get(sender).copied() {
            if let Some(root) = self.super_root_id {
                let sender_hash = Self::hash_peer_id(&format!("{:?}", sender_id));
                if sender_id != root && sender_hash != update.super_root_hash {
                    return;
                }
            }
        }

        let prev_parent = self.relay_parent;
        let prev_backup = self.relay_backup_parent;
        let prev_active = self.relay_active_parent;
        let prev_fanout = self.relay_fanout;
        self.relay_epoch = update.epoch;
        self.super_root_id = self.hash_to_matchbox(update.super_root_hash);
        self.supernode_id = self.super_root_id;
        self.supernode_set = update
            .supernode_hashes
            .iter()
            .filter_map(|hash| self.hash_to_matchbox(*hash))
            .collect();
        self.relay_parent = if update.parent_hash == 0 {
            None
        } else {
            self.hash_to_matchbox(update.parent_hash)
        };
        self.relay_backup_parent = if update.backup_parent_hash == 0 {
            None
        } else {
            self.hash_to_matchbox(update.backup_parent_hash)
        };
        let parent_changed = prev_parent != self.relay_parent;
        if parent_changed && prev_parent.is_some() && prev_parent != self.relay_parent {
            self.relay_backup_parent = prev_parent;
        }
        self.relay_children = update
            .child_hashes
            .iter()
            .filter_map(|hash| self.hash_to_matchbox(*hash))
            .collect();
        self.relay_fanout = (update.fanout as usize).clamp(Self::MIN_FANOUT, Self::MAX_FANOUT);
        self.is_host = self.local_peer_id.is_some() && self.local_peer_id == self.super_root_id;
        if self.is_host {
            self.relay_active_parent = None;
        } else if self.relay_active_parent.is_none() {
            self.relay_active_parent = self.relay_parent;
        } else if let Some(active) = self.relay_active_parent {
            let valid = Some(active) == self.relay_parent || Some(active) == self.relay_backup_parent;
            if !valid {
                self.relay_active_parent = self.relay_parent;
            }
        }
        if prev_parent != self.relay_parent
            || prev_backup != self.relay_backup_parent
            || prev_active != self.relay_active_parent
            || prev_fanout != self.relay_fanout
        {
            self.last_parent_switch_frame = current_frame;
        }
    }

    fn apply_area_authority_update(&mut self, sender: &str, update: AreaAuthorityUpdate) {
        if self.relay_epoch > update.epoch {
            return;
        }
        if let Some(root) = self.super_root_id {
            if let Some(sender_id) = self.peer_id_lookup.get(sender) {
                if *sender_id != root {
                    return;
                }
            }
        }
        self.area_authorities = update
            .entries
            .into_iter()
            .map(|entry| (entry.area_id, entry.authority_hash))
            .collect();
    }

    fn broadcast_topology_update(&mut self) {
        if !self.is_host {
            return;
        }
        let connected_peers: Vec<matchbox_socket::PeerId> = {
            let socket = match &mut self.socket {
                Some(s) => s,
                None => return,
            };
            socket.connected_peers().collect()
        };
        let local_id = match self.local_peer_id {
            Some(id) => id,
            None => return,
        };
        let mut all_nodes: Vec<matchbox_socket::PeerId> = connected_peers.clone();
        all_nodes.push(local_id);
        all_nodes.sort();
        all_nodes.dedup();
        if all_nodes.is_empty() {
            return;
        }

        let mut supernodes = self.supernode_set.clone();
        supernodes.retain(|id| all_nodes.contains(id));
        supernodes.sort();
        if supernodes.is_empty() {
            let mut active_areas: HashSet<u32> = HashSet::new();
            for node in &all_nodes {
                let area = self.peer_pos(*node).map(Self::area_id_from_pos).unwrap_or(0);
                active_areas.insert(area);
            }
            let target_k = Self::choose_dynamic_supernode_count(all_nodes.len(), active_areas.len());
            supernodes = self.select_supernodes_dynamic(&all_nodes, target_k);
            if supernodes.is_empty() {
                supernodes.push(all_nodes[0]);
            }
        }
        let super_root = if let Some(root) = self.super_root_id {
            if supernodes.contains(&root) {
                root
            } else {
                supernodes[0]
            }
        } else {
            supernodes[0]
        };
        let relay_order = self.build_clustered_relay_order(&all_nodes, &supernodes, super_root);
        let relay_fanout = Self::choose_dynamic_fanout(all_nodes.len());
        let mut index_by_id: HashMap<matchbox_socket::PeerId, usize> = HashMap::new();
        for (idx, id) in relay_order.iter().enumerate() {
            index_by_id.insert(*id, idx);
        }

        let super_root_hash = Self::hash_peer_id(&format!("{:?}", super_root));
        let supernode_hashes: Vec<u64> = supernodes
            .iter()
            .map(|id| Self::hash_peer_id(&format!("{:?}", id)))
            .collect();

        let socket = match &mut self.socket {
            Some(s) => s,
            None => return,
        };
        for peer_id in connected_peers {
            let local_idx = match index_by_id.get(&peer_id) {
                Some(idx) => *idx,
                None => continue,
            };
            let (parent, backup_parent, children) =
                Self::tree_assignment_for_index(&relay_order, local_idx, relay_fanout);
            let update = TopologyUpdate {
                epoch: self.relay_epoch,
                super_root_hash,
                supernode_hashes: supernode_hashes.clone(),
                fanout: relay_fanout as u8,
                parent_hash: parent
                    .map(|id| Self::hash_peer_id(&format!("{:?}", id)))
                    .unwrap_or(0),
                backup_parent_hash: backup_parent
                    .filter(|id| Some(*id) != parent)
                    .map(|id| Self::hash_peer_id(&format!("{:?}", id)))
                    .unwrap_or(0),
                child_hashes: children
                    .iter()
                    .map(|id| Self::hash_peer_id(&format!("{:?}", id)))
                    .collect(),
            };
            let msg = NetMessage::TopologyUpdateEvent(update).to_bytes();
            socket.send(msg.into_boxed_slice(), peer_id);
        }
    }

    fn broadcast_area_authorities(&mut self) {
        if !self.is_host {
            return;
        }
        let socket = match &mut self.socket {
            Some(s) => s,
            None => return,
        };
        let entries: Vec<AreaAuthorityEntry> = self
            .area_authorities
            .iter()
            .map(|(area_id, authority_hash)| AreaAuthorityEntry {
                area_id: *area_id,
                authority_hash: *authority_hash,
            })
            .collect();
        let update = AreaAuthorityUpdate {
            epoch: self.relay_epoch,
            entries,
        };
        let msg = NetMessage::AreaAuthorityUpdateEvent(update).to_bytes();
        for peer_id in socket.connected_peers().collect::<Vec<_>>() {
            socket.send(msg.clone().into_boxed_slice(), peer_id);
        }
    }

    pub fn mark_enemy_sync_received(&mut self, current_frame: u32) {
        self.last_enemy_sync_frame = current_frame;
    }

    pub fn supernode_is_stale(&self, current_frame: u32) -> bool {
        if current_frame.saturating_sub(self.last_enemy_sync_frame) < 180 {
            return false;
        }
        if let Some(parent) = self.relay_active_parent.or(self.relay_parent) {
            return !self.peer_seen_recently(parent, current_frame);
        }
        true
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

    fn is_parent_sender(&self, peer_id: &str) -> bool {
        match (self.relay_active_parent.or(self.relay_parent), self.peer_id_lookup.get(peer_id)) {
            (Some(parent), Some(sender)) => *sender == parent,
            _ => false,
        }
    }

    fn is_child_sender(&self, peer_id: &str) -> bool {
        let sender = match self.peer_id_lookup.get(peer_id) {
            Some(id) => *id,
            None => return false,
        };
        self.relay_children.contains(&sender)
    }

    fn relay_parent_or_root(&self) -> Option<matchbox_socket::PeerId> {
        if let Some(parent) = self.relay_active_parent.or(self.relay_parent) {
            Some(parent)
        } else {
            self.super_root_id
        }
    }

    fn upstream_targets(&self) -> Vec<matchbox_socket::PeerId> {
        let mut targets = Vec::new();
        let primary = self.relay_parent;
        let preferred = self
            .relay_active_parent
            .or(primary)
            .or(self.super_root_id)
            .filter(|target| Some(*target) != self.local_peer_id);
        if let Some(target) = preferred {
            targets.push(target);
        }

        // During parent handoff, send to both active and primary for a short window.
        let within_handoff_window =
            self.last_update_frame.saturating_sub(self.last_parent_switch_frame)
                <= Self::RELAY_HANDOFF_DUPLEX_FRAMES;
        if within_handoff_window {
            if let Some(primary_target) = primary.filter(|target| Some(*target) != self.local_peer_id) {
                if !targets.contains(&primary_target) {
                    targets.push(primary_target);
                }
            }
        }
        targets
    }

    fn is_authoritative_sender(&self, peer_id: &str) -> bool {
        self.supernode_id.is_none() || self.is_supernode_sender(peer_id)
    }

    fn low_priority_stride(&self) -> u32 {
        match self.relay_congestion_level() {
            0 => 1,
            1 => 3,
            _ => 6,
        }
    }

    fn allow_low_priority_topic(&mut self, topic: LowPriorityTopic) -> bool {
        let frame = self.last_update_frame;
        let stride = self.low_priority_stride();
        let slot = match topic {
            LowPriorityTopic::Chat => &mut self.last_lowpri_chat_frame,
            LowPriorityTopic::Vote => &mut self.last_lowpri_vote_frame,
            LowPriorityTopic::Ack => &mut self.last_lowpri_ack_frame,
        };
        if *slot == u32::MAX || frame.saturating_sub(*slot) >= stride {
            *slot = frame;
            true
        } else {
            self.relay_telemetry.dropped_messages = self.relay_telemetry.dropped_messages.saturating_add(1);
            false
        }
    }

    fn send_upstream_or_broadcast(&mut self, msg: Vec<u8>) {
        let targets = self.upstream_targets();
        let socket = match &mut self.socket {
            Some(s) => s,
            None => return,
        };
        if !targets.is_empty() {
            for target in targets {
                socket.send(msg.clone().into_boxed_slice(), target);
                self.relay_telemetry.sent_upstream = self.relay_telemetry.sent_upstream.saturating_add(1);
            }
            return;
        }
        for peer_id in socket.connected_peers().collect::<Vec<_>>() {
            socket.send(msg.clone().into_boxed_slice(), peer_id);
            self.relay_telemetry.sent_broadcast = self.relay_telemetry.sent_broadcast.saturating_add(1);
        }
    }

    fn send_low_priority_upstream_or_broadcast(&mut self, topic: LowPriorityTopic, msg: Vec<u8>) {
        if self.allow_low_priority_topic(topic) {
            self.send_upstream_or_broadcast(msg);
        }
    }

    fn send_downstream_or_broadcast(
        &mut self,
        msg: Vec<u8>,
        exclude: Option<matchbox_socket::PeerId>,
    ) {
        let socket = match &mut self.socket {
            Some(s) => s,
            None => return,
        };
        if self.relay_children.is_empty() {
            for peer_id in socket.connected_peers().collect::<Vec<_>>() {
                if Some(peer_id) != exclude {
                    socket.send(msg.clone().into_boxed_slice(), peer_id);
                    self.relay_telemetry.sent_broadcast = self.relay_telemetry.sent_broadcast.saturating_add(1);
                }
            }
            return;
        }
        for peer_id in &self.relay_children {
            if Some(*peer_id) != exclude {
                socket.send(msg.clone().into_boxed_slice(), *peer_id);
                self.relay_telemetry.sent_downstream = self.relay_telemetry.sent_downstream.saturating_add(1);
            }
        }
    }

    fn send_low_priority_downstream_or_broadcast(
        &mut self,
        topic: LowPriorityTopic,
        msg: Vec<u8>,
        exclude: Option<matchbox_socket::PeerId>,
    ) {
        if self.allow_low_priority_topic(topic) {
            self.send_downstream_or_broadcast(msg, exclude);
        }
    }

    fn relay_control_message(&mut self, sender: &str, msg: Vec<u8>) {
        let sender_matchbox = self.peer_id_lookup.get(sender).copied();
        let from_parent = self.is_parent_sender(sender);
        let from_child = self.is_child_sender(sender);

        if from_parent {
            if !self.relay_children.is_empty() {
                self.send_downstream_or_broadcast(msg, sender_matchbox);
            }
            return;
        }

        if from_child {
            if self.relay_parent_or_root().is_some() {
                self.send_upstream_or_broadcast(msg);
            } else if !self.relay_children.is_empty() {
                self.send_downstream_or_broadcast(msg, sender_matchbox);
            }
            return;
        }

        if self.is_host {
            self.send_downstream_or_broadcast(msg, sender_matchbox);
        } else {
            self.send_upstream_or_broadcast(msg);
        }
    }

    fn relay_low_priority_control_message(
        &mut self,
        sender: &str,
        topic: LowPriorityTopic,
        msg: Vec<u8>,
    ) {
        let sender_matchbox = self.peer_id_lookup.get(sender).copied();
        let from_parent = self.is_parent_sender(sender);
        let from_child = self.is_child_sender(sender);

        if from_parent {
            if !self.relay_children.is_empty() {
                self.send_low_priority_downstream_or_broadcast(topic, msg, sender_matchbox);
            }
            return;
        }

        if from_child {
            if self.relay_parent_or_root().is_some() {
                self.send_low_priority_upstream_or_broadcast(topic, msg);
            } else if !self.relay_children.is_empty() {
                self.send_low_priority_downstream_or_broadcast(topic, msg, sender_matchbox);
            }
            return;
        }

        if self.is_host {
            self.send_low_priority_downstream_or_broadcast(topic, msg, sender_matchbox);
        } else {
            self.send_low_priority_upstream_or_broadcast(topic, msg);
        }
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

    pub fn paid_obstacle_has_supernode_ack(&self, proof_hash: [u8; 32]) -> bool {
        let supernode = match self.supernode_id {
            Some(id) => id,
            None => return true,
        };
        self.paid_obstacle_confirmations
            .get(&proof_hash)
            .map(|set| set.contains(&supernode))
            .unwrap_or(false)
    }

    pub fn record_paid_ability_confirmation(&mut self, proof_hash: [u8; 32], peer_id: matchbox_socket::PeerId) -> usize {
        let entry = self.paid_ability_confirmations.entry(proof_hash).or_default();
        entry.insert(peer_id);
        entry.len()
    }

    pub fn paid_ability_confirmation_count(&self, proof_hash: [u8; 32]) -> usize {
        self.paid_ability_confirmations
            .get(&proof_hash)
            .map(|set| set.len())
            .unwrap_or(0)
    }

    pub fn paid_ability_has_supernode_ack(&self, proof_hash: [u8; 32]) -> bool {
        let supernode = match self.supernode_id {
            Some(id) => id,
            None => return true,
        };
        self.paid_ability_confirmations
            .get(&proof_hash)
            .map(|set| set.contains(&supernode))
            .unwrap_or(false)
    }

    pub fn resolve_peer_id(&self, peer_id: &str) -> Option<matchbox_socket::PeerId> {
        self.peer_id_lookup.get(peer_id).copied()
    }

    pub fn resolve_peer_hash(&self, hash: u64) -> Option<PeerId> {
        self.peer_hash_lookup.get(&hash).cloned()
    }

    fn hash_peer_id(peer_id: &str) -> u64 {
        let mut hash: u64 = 0xcbf29ce484222325;
        for b in peer_id.as_bytes() {
            hash ^= *b as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash
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

    fn log_telemetry_periodic(&mut self, current_frame: u32) {
        if current_frame.saturating_sub(self.last_telemetry_log_frame) < 600 {
            return;
        }
        if self.remote_players.len() + 1 < 6 {
            return;
        }
        self.last_telemetry_log_frame = current_frame;
        web_sys::console::log_1(
            &format!(
                "[net] peers={} q={} lvl={} rx={} up={} dn={} bc={} drop={} qdrop={} maxq={} sw={}",
                self.remote_players.len() + 1,
                self.relay_queue_depth(),
                self.relay_congestion_level(),
                self.relay_telemetry.recv_messages,
                self.relay_telemetry.sent_upstream,
                self.relay_telemetry.sent_downstream,
                self.relay_telemetry.sent_broadcast,
                self.relay_telemetry.dropped_messages,
                self.relay_telemetry.dropped_queue_entries,
                self.relay_telemetry.max_queue_depth,
                self.relay_telemetry.stale_parent_switches,
            )
            .into(),
        );
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

    pub fn send_chat_message(&mut self, chat: ChatMessage) {
        let msg = NetMessage::ChatMessageEvent(chat).to_bytes();
        if self.is_host {
            self.send_low_priority_downstream_or_broadcast(LowPriorityTopic::Chat, msg, None);
        } else {
            self.send_low_priority_upstream_or_broadcast(LowPriorityTopic::Chat, msg);
        }
    }

    pub fn send_vote_mute(&mut self, vote: VoteMute) {
        let msg = NetMessage::VoteMuteEvent(vote).to_bytes();
        if self.is_host {
            self.send_low_priority_downstream_or_broadcast(LowPriorityTopic::Vote, msg, None);
        } else {
            self.send_low_priority_upstream_or_broadcast(LowPriorityTopic::Vote, msg);
        }
    }

    pub fn take_chat_messages(&mut self) -> Vec<ChatMessage> {
        std::mem::take(&mut self.pending_chat_messages)
    }

    pub fn take_vote_mutes(&mut self) -> Vec<VoteMute> {
        std::mem::take(&mut self.pending_vote_mutes)
    }

    pub fn is_muted(&self, hash: u64) -> bool {
        self.muted_hashes.contains(&hash)
    }

    pub fn register_vote_mute(&mut self, vote: VoteMute) -> bool {
        if vote.target_hash == 0
            || vote.voter_hash == 0
            || vote.target_hash == vote.voter_hash
        {
            return false;
        }

        let entry = self.vote_mutes.entry(vote.target_hash).or_default();
        entry.insert(vote.voter_hash);
        let total_players = self.remote_players.len() + 1;
        let required = total_players / 2 + 1;
        if entry.len() >= required {
            self.muted_hashes.insert(vote.target_hash);
            return true;
        }
        false
    }

    pub fn mute_locally(&mut self, hash: u64) {
        if hash != 0 {
            self.muted_hashes.insert(hash);
        }
    }

    pub fn resolve_hash_by_name(&self, name: &str) -> Option<u64> {
        let query = name.trim().to_ascii_lowercase();
        if query.is_empty() {
            return None;
        }

        if self.local_player_name.to_ascii_lowercase() == query {
            return self.local_peer_hash;
        }

        for (peer_id, remote) in &self.remote_players {
            if remote.name.to_ascii_lowercase() == query {
                return Some(Self::hash_peer_id(peer_id));
            }
        }

        None
    }

    pub fn display_name_for_hash(&self, hash: u64) -> String {
        if let Some(local_hash) = self.local_peer_hash {
            if local_hash == hash {
                return self.local_player_name.clone();
            }
        }

        if let Some(peer_id) = self.peer_hash_lookup.get(&hash) {
            if let Some(remote) = self.remote_players.get(peer_id) {
                return remote.name.clone();
            }
        }

        "Player".to_string()
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

    pub fn record_local_kill(&mut self, enemy_type: crate::net::EnemyType) {
        self.local_stats.kills = self.local_stats.kills.saturating_add(1);
        match enemy_type {
            crate::net::EnemyType::Spider => {
                self.local_stats.spider_kills = self.local_stats.spider_kills.saturating_add(1);
            }
            crate::net::EnemyType::Cannon => {
                self.local_stats.cannon_kills = self.local_stats.cannon_kills.saturating_add(1);
            }
            crate::net::EnemyType::Snake => {
                self.local_stats.snake_kills = self.local_stats.snake_kills.saturating_add(1);
            }
            crate::net::EnemyType::Wisp => {
                self.local_stats.wisp_kills = self.local_stats.wisp_kills.saturating_add(1);
            }
            crate::net::EnemyType::Guardian => {}
        }
    }

    pub fn record_local_deaths(&mut self, count: u32) {
        self.local_stats.deaths = self.local_stats.deaths.saturating_add(count);
    }

    pub fn record_local_attack_attempts(&mut self, count: u32) {
        if count > 0 {
            self.local_stats.attack_attempts = self.local_stats.attack_attempts.saturating_add(count);
        }
    }

    pub fn record_local_attack_hits(&mut self, count: u32) {
        if count > 0 {
            self.local_stats.attack_hits = self.local_stats.attack_hits.saturating_add(count);
        }
    }

    pub fn record_remote_kill(&mut self, peer_id: &PeerId, enemy_type: crate::net::EnemyType) {
        let stats = self.remote_stats.entry(peer_id.clone()).or_default();
        stats.kills = stats.kills.saturating_add(1);
        match enemy_type {
            crate::net::EnemyType::Spider => {
                stats.spider_kills = stats.spider_kills.saturating_add(1);
            }
            crate::net::EnemyType::Cannon => {
                stats.cannon_kills = stats.cannon_kills.saturating_add(1);
            }
            crate::net::EnemyType::Snake => {
                stats.snake_kills = stats.snake_kills.saturating_add(1);
            }
            crate::net::EnemyType::Wisp => {
                stats.wisp_kills = stats.wisp_kills.saturating_add(1);
            }
            crate::net::EnemyType::Guardian => {}
        }
    }

    pub fn record_remote_death(&mut self, peer_id: &PeerId, count: u32) {
        let stats = self.remote_stats.entry(peer_id.clone()).or_default();
        stats.deaths = stats.deaths.saturating_add(count);
    }

    pub fn room_totals(&self) -> PlayerStats {
        let mut totals = self.local_stats.clone();
        for stats in self.remote_stats.values() {
            totals.kills = totals.kills.saturating_add(stats.kills);
            totals.spider_kills = totals.spider_kills.saturating_add(stats.spider_kills);
            totals.cannon_kills = totals.cannon_kills.saturating_add(stats.cannon_kills);
            totals.snake_kills = totals.snake_kills.saturating_add(stats.snake_kills);
            totals.wisp_kills = totals.wisp_kills.saturating_add(stats.wisp_kills);
            totals.attack_attempts = totals.attack_attempts.saturating_add(stats.attack_attempts);
            totals.attack_hits = totals.attack_hits.saturating_add(stats.attack_hits);
            totals.deaths = totals.deaths.saturating_add(stats.deaths);
            totals.time_played_frames = totals.time_played_frames.saturating_add(stats.time_played_frames);
        }
        totals
    }

    pub fn score_for_stats(&self, stats: &PlayerStats) -> u32 {
        let spider = stats.spider_kills as f32;
        let cannon = stats.cannon_kills as f32;
        let snake = stats.snake_kills as f32;
        let wisp = stats.wisp_kills as f32;
        let weighted_kills = spider * 1.0 + cannon * 2.5 + snake * 4.0 + wisp * 1.5;
        let deaths = stats.deaths as f32;
        let time_seconds = stats.time_seconds() as f32;
        let time_minutes = time_seconds / 60.0;

        let activity = weighted_kills / (time_minutes + 1.0);
        let survival = 1.0 / (1.0 + deaths * 0.5);
        let stability = survival * 20.0 * (weighted_kills / (weighted_kills + 5.0));
        let base = weighted_kills * 25.0;
        let tempo = activity * 10.0;
        let penalty = deaths * 120.0;
        let time_decay = 1.0 / (1.0 + time_minutes * 0.4);

        let attempts = stats.attack_attempts as f32;
        let hits = stats.attack_hits as f32;
        let accuracy = if attempts > 0.0 {
            (hits / attempts).clamp(0.1, 2.0)
        } else {
            1.0
        };
        let accuracy_multiplier = (0.5 + 0.5 * accuracy).clamp(0.5, 1.5);

        let raw = (base + tempo + stability - penalty).max(0.0) * time_decay * accuracy_multiplier;
        raw.round().max(0.0) as u32
    }

    fn queue_relay_player_state(&mut self, peer_hash: u64, state: PlayerState) {
        self.relay_player_states.insert(peer_hash, state);
    }

    fn queue_relay_input_frame(&mut self, peer_hash: u64, frame: InputFrame) {
        let area_id = self.area_id_for_hash(peer_hash);
        if self.relay_input_frames.len() >= Self::MAX_RELAY_INPUT_QUEUE {
            self.relay_input_frames.remove(0);
            self.relay_telemetry.dropped_queue_entries = self.relay_telemetry.dropped_queue_entries.saturating_add(1);
        }
        self.relay_input_frames.push(InputFrameEntry { peer_hash, area_id, frame });
    }

    fn queue_downlink_player_batch(&mut self, batch: PlayerStateBatch) {
        if self.downlink_player_batches.len() >= Self::MAX_DOWNLINK_QUEUE {
            let dropped = self
                .downlink_player_batches
                .first()
                .map(|b| b.entries.len() as u32)
                .unwrap_or(0);
            if !self.downlink_player_batches.is_empty() {
                self.downlink_player_batches.remove(0);
            }
            self.relay_telemetry.dropped_queue_entries = self
                .relay_telemetry
                .dropped_queue_entries
                .saturating_add(dropped.max(1));
        }
        self.downlink_player_batches.push(batch);
    }

    fn queue_downlink_input_batch(&mut self, batch: InputFrameBatch) {
        if self.downlink_input_batches.len() >= Self::MAX_DOWNLINK_QUEUE {
            let dropped = self
                .downlink_input_batches
                .first()
                .map(|b| b.entries.len() as u32)
                .unwrap_or(0);
            if !self.downlink_input_batches.is_empty() {
                self.downlink_input_batches.remove(0);
            }
            self.relay_telemetry.dropped_queue_entries = self
                .relay_telemetry
                .dropped_queue_entries
                .saturating_add(dropped.max(1));
        }
        self.downlink_input_batches.push(batch);
    }

    pub fn flush_relay_batches(&mut self) {
        let parent = self.relay_active_parent.or(self.relay_parent);
        let children = self.relay_children.clone();
        let hash_lookup = self.peer_hash_lookup.clone();
        let mut remote_positions: HashMap<PeerId, Vec2> = self
            .remote_players
            .iter()
            .map(|(id, remote)| (id.clone(), remote.pos))
            .collect();
        if let (Some(local_hash), Some(local_pos)) = (self.local_peer_hash, self.local_last_pos) {
            let local_id = self
                .local_peer_id
                .map(|id| format!("{:?}", id))
                .unwrap_or_default();
            if !local_id.is_empty() {
                remote_positions.insert(local_id.clone(), local_pos);
                let _ = hash_lookup.get(&local_hash);
            }
        }

        let mut id_lookup: HashMap<matchbox_socket::PeerId, PeerId> = HashMap::new();
        for (peer_id, matchbox_id) in &self.peer_id_lookup {
            id_lookup.insert(*matchbox_id, peer_id.clone());
        }

        let queue_depth = self.relay_player_states.len()
            + self.relay_input_frames.len()
            + self.downlink_player_batches.iter().map(|b| b.entries.len()).sum::<usize>()
            + self.downlink_input_batches.iter().map(|b| b.entries.len()).sum::<usize>();
        if queue_depth > self.relay_telemetry.max_queue_depth {
            self.relay_telemetry.max_queue_depth = queue_depth;
        }
        let batch_cap = match queue_depth {
            d if d >= 900 => 256,
            d if d >= 400 => 384,
            _ => Self::MAX_BATCH_ENTRIES,
        };

        let socket = match &mut self.socket {
            Some(s) => s,
            None => return,
        };

        if !self.relay_player_states.is_empty() {
            let mut entries: Vec<PlayerStateEntry> = self
                .relay_player_states
                .drain()
                .map(|(peer_hash, state)| {
                    let area_id = Self::area_id_from_pos(Vec2::new(state.x, state.y));
                    PlayerStateEntry { peer_hash, area_id, state }
                })
                .collect();
            if entries.len() > batch_cap {
                let dropped = (entries.len() - batch_cap) as u32;
                entries.truncate(batch_cap);
                self.relay_telemetry.dropped_queue_entries = self
                    .relay_telemetry
                    .dropped_queue_entries
                    .saturating_add(dropped);
            }
            let radius_sq = Self::interest_radius_sq();
            if let Some(parent_peer) = parent {
                let msg = NetMessage::PlayerStateBatchEvent(PlayerStateBatch { entries }).to_bytes();
                socket.send(msg.into_boxed_slice(), parent_peer);
            } else if !children.is_empty() {
                for child in &children {
                    let child_id = match id_lookup.get(child) {
                        Some(id) => id.clone(),
                        None => continue,
                    };
                    let target_hash = Self::hash_peer_id(&child_id);
                    let target_pos = remote_positions.get(&child_id).copied();
                    let target_area = target_pos.map(Self::area_id_from_pos);
                    let mut filtered: Vec<PlayerStateEntry> = entries
                        .iter()
                        .filter(|entry| {
                            if entry.peer_hash == target_hash {
                                return true;
                            }
                            if let Some(area) = target_area {
                                if entry.area_id == area {
                                    return true;
                                }
                            }
                            if let Some(pos) = target_pos {
                                let dx = entry.state.x - pos.x;
                                let dy = entry.state.y - pos.y;
                                return dx * dx + dy * dy <= radius_sq;
                            }
                            true
                        })
                        .cloned()
                        .collect();
                    if filtered.len() > batch_cap {
                        let dropped = (filtered.len() - batch_cap) as u32;
                        filtered.truncate(batch_cap);
                        self.relay_telemetry.dropped_queue_entries = self
                            .relay_telemetry
                            .dropped_queue_entries
                            .saturating_add(dropped);
                    }
                    if filtered.is_empty() {
                        continue;
                    }
                    let msg = NetMessage::PlayerStateBatchEvent(PlayerStateBatch { entries: filtered }).to_bytes();
                    socket.send(msg.into_boxed_slice(), *child);
                }
            }
        }

        if !self.downlink_player_batches.is_empty() && !children.is_empty() {
            let radius_sq = Self::interest_radius_sq();
            let batches = std::mem::take(&mut self.downlink_player_batches);
            for batch in batches {
                for child in &children {
                    let child_id = match id_lookup.get(child) {
                        Some(id) => id.clone(),
                        None => continue,
                    };
                    let target_hash = Self::hash_peer_id(&child_id);
                    let target_pos = remote_positions.get(&child_id).copied();
                    let target_area = target_pos.map(Self::area_id_from_pos);
                    let mut filtered: Vec<PlayerStateEntry> = batch
                        .entries
                        .iter()
                        .filter(|entry| {
                            if entry.peer_hash == target_hash {
                                return true;
                            }
                            if let Some(area) = target_area {
                                if entry.area_id == area {
                                    return true;
                                }
                            }
                            if let Some(pos) = target_pos {
                                let dx = entry.state.x - pos.x;
                                let dy = entry.state.y - pos.y;
                                return dx * dx + dy * dy <= radius_sq;
                            }
                            true
                        })
                        .cloned()
                        .collect();
                    if filtered.len() > batch_cap {
                        let dropped = (filtered.len() - batch_cap) as u32;
                        filtered.truncate(batch_cap);
                        self.relay_telemetry.dropped_queue_entries = self
                            .relay_telemetry
                            .dropped_queue_entries
                            .saturating_add(dropped);
                    }
                    if filtered.is_empty() {
                        continue;
                    }
                    let msg = NetMessage::PlayerStateBatchEvent(PlayerStateBatch { entries: filtered }).to_bytes();
                    socket.send(msg.into_boxed_slice(), *child);
                }
            }
        } else {
            self.downlink_player_batches.clear();
        }

        if !self.relay_input_frames.is_empty() {
            let mut entries = std::mem::take(&mut self.relay_input_frames);
            if entries.len() > batch_cap {
                let drop = entries.len() - batch_cap;
                entries = entries.split_off(drop);
                self.relay_telemetry.dropped_queue_entries = self
                    .relay_telemetry
                    .dropped_queue_entries
                    .saturating_add(drop as u32);
            }
            let radius_sq = Self::interest_radius_sq();
            if let Some(parent_peer) = parent {
                let msg = NetMessage::InputFrameBatchEvent(InputFrameBatch { entries }).to_bytes();
                socket.send(msg.into_boxed_slice(), parent_peer);
            } else if !children.is_empty() {
                for child in &children {
                    let child_id = match id_lookup.get(child) {
                        Some(id) => id.clone(),
                        None => continue,
                    };
                    let target_hash = Self::hash_peer_id(&child_id);
                    let target_pos = remote_positions.get(&child_id).copied();
                    let target_area = target_pos.map(Self::area_id_from_pos);
                    let mut filtered: Vec<InputFrameEntry> = entries
                        .iter()
                        .filter(|entry| {
                            if entry.peer_hash == target_hash {
                                return true;
                            }
                            if let Some(area) = target_area {
                                if entry.area_id == area {
                                    return true;
                                }
                            }
                            let entry_peer_id = match hash_lookup.get(&entry.peer_hash) {
                                Some(id) => id,
                                None => return true,
                            };
                            match remote_positions.get(entry_peer_id) {
                                Some(pos) => {
                                    if let Some(target) = target_pos {
                                        let dx = pos.x - target.x;
                                        let dy = pos.y - target.y;
                                        dx * dx + dy * dy <= radius_sq
                                    } else {
                                        true
                                    }
                                }
                                None => true,
                            }
                        })
                        .cloned()
                        .collect();
                    if filtered.len() > batch_cap {
                        let dropped = (filtered.len() - batch_cap) as u32;
                        filtered.truncate(batch_cap);
                        self.relay_telemetry.dropped_queue_entries = self
                            .relay_telemetry
                            .dropped_queue_entries
                            .saturating_add(dropped);
                    }
                    if filtered.is_empty() {
                        continue;
                    }
                    let msg = NetMessage::InputFrameBatchEvent(InputFrameBatch { entries: filtered }).to_bytes();
                    socket.send(msg.into_boxed_slice(), *child);
                }
            }
        }

        if !self.downlink_input_batches.is_empty() && !children.is_empty() {
            let radius_sq = Self::interest_radius_sq();
            let batches = std::mem::take(&mut self.downlink_input_batches);
            for batch in batches {
                for child in &children {
                    let child_id = match id_lookup.get(child) {
                        Some(id) => id.clone(),
                        None => continue,
                    };
                    let target_hash = Self::hash_peer_id(&child_id);
                    let target_pos = remote_positions.get(&child_id).copied();
                    let target_area = target_pos.map(Self::area_id_from_pos);
                    let mut filtered: Vec<InputFrameEntry> = batch
                        .entries
                        .iter()
                        .filter(|entry| {
                            if entry.peer_hash == target_hash {
                                return true;
                            }
                            if let Some(area) = target_area {
                                if entry.area_id == area {
                                    return true;
                                }
                            }
                            let entry_peer_id = match hash_lookup.get(&entry.peer_hash) {
                                Some(id) => id,
                                None => return true,
                            };
                            match remote_positions.get(entry_peer_id) {
                                Some(pos) => {
                                    if let Some(target) = target_pos {
                                        let dx = pos.x - target.x;
                                        let dy = pos.y - target.y;
                                        dx * dx + dy * dy <= radius_sq
                                    } else {
                                        true
                                    }
                                }
                                None => true,
                            }
                        })
                        .cloned()
                        .collect();
                    if filtered.len() > batch_cap {
                        let dropped = (filtered.len() - batch_cap) as u32;
                        filtered.truncate(batch_cap);
                        self.relay_telemetry.dropped_queue_entries = self
                            .relay_telemetry
                            .dropped_queue_entries
                            .saturating_add(dropped);
                    }
                    if filtered.is_empty() {
                        continue;
                    }
                    let msg = NetMessage::InputFrameBatchEvent(InputFrameBatch { entries: filtered }).to_bytes();
                    socket.send(msg.into_boxed_slice(), *child);
                }
            }
        } else {
            self.downlink_input_batches.clear();
        }
    }

    /// Send local player state
    pub fn send_player_state(&mut self, state: PlayerState) {
        self.local_last_pos = Some(state.pos());
        if self.is_host {
            if let Some(hash) = self.local_peer_hash {
                self.queue_relay_player_state(hash, state);
            }
            return;
        }

        let area_id = Self::area_id_from_pos(state.pos());
        let target = self
            .area_authority_for(area_id)
            .or(self.relay_parent_or_root())
            .filter(|target| Some(*target) != self.local_peer_id);
        let socket = match &mut self.socket {
            Some(s) => s,
            None => return,
        };
        let msg = NetMessage::PlayerUpdate(state).to_bytes();
        if let Some(target) = target {
            socket.send(msg.into_boxed_slice(), target);
        } else {
            for peer_id in socket.connected_peers().collect::<Vec<_>>() {
                socket.send(msg.clone().into_boxed_slice(), peer_id);
            }
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

    pub fn relay_queue_depth(&self) -> usize {
        self.relay_player_states.len()
            + self.relay_input_frames.len()
            + self.downlink_player_batches.iter().map(|b| b.entries.len()).sum::<usize>()
            + self.downlink_input_batches.iter().map(|b| b.entries.len()).sum::<usize>()
    }

    pub fn relay_congestion_level(&self) -> u8 {
        let depth = self.relay_queue_depth();
        if depth >= 900 {
            2
        } else if depth >= 400 {
            1
        } else {
            0
        }
    }

    pub fn relay_fanout(&self) -> usize {
        self.relay_fanout
    }

    /// Get number of connected peers
    pub fn peer_count(&self) -> usize {
        self.remote_players.len()
    }

    pub fn known_peer_count(&self) -> usize {
        self.peer_id_lookup.len()
    }

    pub fn desired_peer_count(&self) -> usize {
        self.desired_peer_set.len()
    }

    pub fn discovery_attached(&self) -> bool {
        self.discovery_attached
    }

    /// Send enemy sync to all peers (host only)
    pub fn send_enemy_sync(&mut self, sync: EnemySync) {
        if !self.is_host {
            return;
        }
        let msg = NetMessage::EnemySync(sync).to_bytes();
        self.send_downstream_or_broadcast(msg, None);
    }

    fn forward_enemy_sync_down(&mut self, sync: EnemySync) {
        if self.relay_children.is_empty() {
            return;
        }
        let msg = NetMessage::EnemySync(sync).to_bytes();
        self.send_downstream_or_broadcast(msg, None);
    }

    fn relay_enemy_kill(&mut self, kill: EnemyKill, exclude: Option<matchbox_socket::PeerId>) {
        let msg = NetMessage::EnemyKillEvent(kill).to_bytes();
        self.send_downstream_or_broadcast(msg, exclude);
    }

    fn relay_player_death(&mut self, death: PlayerDeath, exclude: Option<matchbox_socket::PeerId>) {
        let msg = NetMessage::PlayerDeathEvent(death).to_bytes();
        self.send_downstream_or_broadcast(msg, exclude);
    }

    fn relay_paid_obstacle(&mut self, obstacle: PaidObstacle, exclude: Option<matchbox_socket::PeerId>) {
        let msg = NetMessage::PaidObstacleEvent(obstacle).to_bytes();
        self.send_downstream_or_broadcast(msg, exclude);
    }

    fn relay_paid_ability(&mut self, ability: PaidAbility, exclude: Option<matchbox_socket::PeerId>) {
        let msg = NetMessage::PaidAbilityEvent(ability).to_bytes();
        self.send_downstream_or_broadcast(msg, exclude);
    }

    /// Send enemy damage event to host (client only)
    pub fn send_enemy_damage(&mut self, damage: EnemyDamage) {
        if self.is_host {
            // Host processes locally, no need to send
            self.pending_enemy_damage.push(damage);
            return;
        }
        let msg = NetMessage::EnemyDamageEvent(damage).to_bytes();
        self.send_upstream_or_broadcast(msg);
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
        let msg = NetMessage::WaveStartEvent(wave_start).to_bytes();
        self.send_downstream_or_broadcast(msg, None);
    }

    fn forward_wave_start_down(&mut self, wave_start: WaveStart) {
        if self.relay_children.is_empty() {
            return;
        }
        let msg = NetMessage::WaveStartEvent(wave_start).to_bytes();
        self.send_downstream_or_broadcast(msg, None);
    }

    /// Send enemy kill event to all peers
    pub fn send_enemy_kill(&mut self, kill: EnemyKill) {
        let msg = NetMessage::EnemyKillEvent(kill).to_bytes();
        if self.is_host {
            self.send_downstream_or_broadcast(msg, None);
        } else {
            self.send_upstream_or_broadcast(msg);
        }
    }

    /// Send player death event to all peers
    pub fn send_player_death(&mut self, death: PlayerDeath) {
        let msg = NetMessage::PlayerDeathEvent(death).to_bytes();
        if self.is_host {
            self.send_downstream_or_broadcast(msg, None);
        } else {
            self.send_upstream_or_broadcast(msg);
        }
    }

    /// Send paid obstacle event to all peers
    pub fn send_paid_obstacle(&mut self, obstacle: PaidObstacle) {
        let msg = NetMessage::PaidObstacleEvent(obstacle).to_bytes();
        if self.is_host {
            self.send_downstream_or_broadcast(msg, None);
        } else {
            self.send_upstream_or_broadcast(msg);
        }
    }

    pub fn send_paid_obstacle_to_supernode(&mut self, obstacle: PaidObstacle) {
        self.send_paid_obstacle(obstacle);
    }

    /// Send paid ability event to all peers
    pub fn send_paid_ability(&mut self, ability: PaidAbility) {
        let msg = NetMessage::PaidAbilityEvent(ability).to_bytes();
        if self.is_host {
            self.send_downstream_or_broadcast(msg, None);
        } else {
            self.send_upstream_or_broadcast(msg);
        }
    }

    pub fn send_paid_ability_to_supernode(&mut self, ability: PaidAbility) {
        self.send_paid_ability(ability);
    }

    pub fn send_paid_obstacle_ack(&mut self, ack: PaidObstacleAck) {
        let msg = NetMessage::PaidObstacleAckEvent(ack).to_bytes();
        if self.is_host {
            self.send_low_priority_downstream_or_broadcast(LowPriorityTopic::Ack, msg, None);
        } else {
            self.send_low_priority_upstream_or_broadcast(LowPriorityTopic::Ack, msg);
        }
    }

    pub fn send_paid_ability_ack(&mut self, ack: PaidAbilityAck) {
        let msg = NetMessage::PaidAbilityAckEvent(ack).to_bytes();
        if self.is_host {
            self.send_low_priority_downstream_or_broadcast(LowPriorityTopic::Ack, msg, None);
        } else {
            self.send_low_priority_upstream_or_broadcast(LowPriorityTopic::Ack, msg);
        }
    }

    pub fn send_cannon_shot(&mut self, shot: CannonShot) {
        if !self.is_host {
            return;
        }
        let msg = NetMessage::CannonShotEvent(shot).to_bytes();
        self.send_downstream_or_broadcast(msg, None);
    }

    pub fn send_input_frame(&mut self, frame: InputFrame) {
        if self.is_host {
            if let Some(hash) = self.local_peer_hash {
                self.queue_relay_input_frame(hash, frame);
            }
            return;
        }

        let area_id = self.local_last_pos.map(Self::area_id_from_pos).unwrap_or(0);
        let target = self
            .area_authority_for(area_id)
            .or(self.relay_parent_or_root())
            .filter(|target| Some(*target) != self.local_peer_id);
        let socket = match &mut self.socket {
            Some(s) => s,
            None => return,
        };
        let msg = NetMessage::InputFrameEvent(frame).to_bytes();
        if let Some(target) = target {
            socket.send(msg.into_boxed_slice(), target);
        } else {
            for peer_id in socket.connected_peers().collect::<Vec<_>>() {
                socket.send(msg.clone().into_boxed_slice(), peer_id);
            }
        }
    }

    /// Take pending wave start (for client to apply)
    pub fn take_wave_start(&mut self) -> Option<WaveStart> {
        self.pending_wave_start.take()
    }

    /// Take pending enemy kills from other players
    pub fn take_enemy_kills_optimistic(&mut self) -> Vec<EnemyKill> {
        std::mem::take(&mut self.pending_enemy_kills_optimistic)
    }

    pub fn take_enemy_kills_confirmed(&mut self) -> Vec<EnemyKill> {
        std::mem::take(&mut self.pending_enemy_kills_confirmed)
    }

    /// Take pending player deaths from other players
    pub fn take_player_deaths_optimistic(&mut self) -> Vec<PlayerDeath> {
        std::mem::take(&mut self.pending_player_deaths_optimistic)
    }

    pub fn take_player_deaths_confirmed(&mut self) -> Vec<PlayerDeath> {
        std::mem::take(&mut self.pending_player_deaths_confirmed)
    }

    fn handle_enemy_kill(&mut self, peer_id: PeerId, kill: EnemyKill, current_frame: u32) {
        if self.applied_event_ids.contains_key(&kill.event_id) {
            return;
        }
        let from_parent = self.is_parent_sender(&peer_id);
        let from_authority = self.is_authoritative_sender(&peer_id);
        let from_child = self.is_child_sender(&peer_id);

        if !self.is_host && (from_authority || from_parent) {
            self.pending_enemy_kills_confirmed.push(kill);
            self.applied_event_ids.insert(kill.event_id, current_frame);
            if from_parent && !self.relay_children.is_empty() {
                self.relay_enemy_kill(kill, self.resolve_peer_id(&peer_id));
            }
            return;
        }
        if !self.is_host && from_child {
            self.send_upstream_or_broadcast(NetMessage::EnemyKillEvent(kill).to_bytes());
        }
        let entry = self
            .enemy_kill_confirmations
            .entry(kill.event_id)
            .or_insert_with(|| (kill, HashSet::new(), current_frame));
        entry.1.insert(peer_id.clone());

        if self.is_host {
            if entry.1.len() == 1 {
                self.relay_enemy_kill(kill, self.resolve_peer_id(&peer_id));
            }
            self.pending_enemy_kills_confirmed.push(kill);
            self.applied_event_ids.insert(kill.event_id, current_frame);
            self.enemy_kill_confirmations.remove(&kill.event_id);
            return;
        }

        if !self.optimistic_enemy_event_ids.contains_key(&kill.event_id) {
            self.pending_enemy_kills_optimistic.push(kill);
            self.optimistic_enemy_event_ids.insert(kill.event_id, current_frame);
        }

        if entry.1.len() >= 2 {
            let event = entry.0;
            self.pending_enemy_kills_confirmed.push(event);
            self.applied_event_ids.insert(event.event_id, current_frame);
            self.optimistic_enemy_event_ids.remove(&event.event_id);
            self.enemy_kill_confirmations.remove(&event.event_id);
        }
    }

    fn handle_player_death(&mut self, peer_id: PeerId, death: PlayerDeath, current_frame: u32) {
        if self.applied_event_ids.contains_key(&death.event_id) {
            return;
        }
        let from_parent = self.is_parent_sender(&peer_id);
        let from_authority = self.is_authoritative_sender(&peer_id);
        let from_child = self.is_child_sender(&peer_id);

        if !self.is_host && (from_authority || from_parent) {
            self.pending_player_deaths_confirmed.push(death);
            self.applied_event_ids.insert(death.event_id, current_frame);
            if from_parent && !self.relay_children.is_empty() {
                self.relay_player_death(death, self.resolve_peer_id(&peer_id));
            }
            return;
        }
        if !self.is_host && from_child {
            self.send_upstream_or_broadcast(NetMessage::PlayerDeathEvent(death).to_bytes());
        }
        let entry = self
            .player_death_confirmations
            .entry(death.event_id)
            .or_insert_with(|| (death, HashSet::new(), current_frame));
        entry.1.insert(peer_id.clone());

        if self.is_host {
            if entry.1.len() == 1 {
                self.relay_player_death(death, self.resolve_peer_id(&peer_id));
            }
            self.pending_player_deaths_confirmed.push(death);
            self.applied_event_ids.insert(death.event_id, current_frame);
            self.player_death_confirmations.remove(&death.event_id);
            return;
        }

        if !self.optimistic_death_event_ids.contains_key(&death.event_id) {
            self.pending_player_deaths_optimistic.push(death);
            self.optimistic_death_event_ids.insert(death.event_id, current_frame);
        }

        if entry.1.len() >= 2 {
            let event = entry.0;
            self.pending_player_deaths_confirmed.push(event);
            self.applied_event_ids.insert(event.event_id, current_frame);
            self.optimistic_death_event_ids.remove(&event.event_id);
            self.player_death_confirmations.remove(&event.event_id);
        }
    }

    fn prune_event_confirmations(&mut self, current_frame: u32) {
        let ttl = 600;
        self.enemy_kill_confirmations
            .retain(|_, (_, _, frame)| current_frame.saturating_sub(*frame) < ttl);
        self.player_death_confirmations
            .retain(|_, (_, _, frame)| current_frame.saturating_sub(*frame) < ttl);
        self.applied_event_ids
            .retain(|_, frame| current_frame.saturating_sub(*frame) < ttl);
        self.optimistic_enemy_event_ids
            .retain(|_, frame| current_frame.saturating_sub(*frame) < ttl);
        self.optimistic_death_event_ids
            .retain(|_, frame| current_frame.saturating_sub(*frame) < ttl);
    }

    /// Take pending paid obstacles from other players
    pub fn take_paid_obstacles(&mut self) -> Vec<(PeerId, PaidObstacle)> {
        std::mem::take(&mut self.pending_paid_obstacles)
    }

    pub fn take_paid_obstacle_acks(&mut self) -> Vec<(PeerId, PaidObstacleAck)> {
        std::mem::take(&mut self.pending_paid_obstacle_acks)
    }

    /// Take pending paid abilities from other players
    pub fn take_paid_abilities(&mut self) -> Vec<(PeerId, PaidAbility)> {
        std::mem::take(&mut self.pending_paid_abilities)
    }

    pub fn take_paid_ability_acks(&mut self) -> Vec<(PeerId, PaidAbilityAck)> {
        std::mem::take(&mut self.pending_paid_ability_acks)
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

    pub fn build_paid_ability(
        &self,
        ability_type: PaidAbilityType,
        x: f32,
        y: f32,
        radius: f32,
        nonce: u32,
    ) -> PaidAbility {
        let proof_hash = Self::compute_paid_ability_hash(
            &self.room_code,
            ability_type,
            x,
            y,
            radius,
            nonce,
        );
        PaidAbility {
            ability_type: ability_type as u8,
            x,
            y,
            radius,
            nonce,
            proof_hash,
        }
    }

    pub fn verify_paid_ability(&self, ability: &PaidAbility) -> bool {
        let ability_type = match PaidAbilityType::from_u8(ability.ability_type) {
            Some(t) => t,
            None => return false,
        };
        let expected = Self::compute_paid_ability_hash(
            &self.room_code,
            ability_type,
            ability.x,
            ability.y,
            ability.radius,
            ability.nonce,
        );
        ability.proof_hash == expected
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

    fn compute_paid_ability_hash(
        room_code: &str,
        ability_type: PaidAbilityType,
        x: f32,
        y: f32,
        radius: f32,
        nonce: u32,
    ) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(room_code.as_bytes());
        hasher.update(&[ability_type as u8]);
        hasher.update(&x.to_le_bytes());
        hasher.update(&y.to_le_bytes());
        hasher.update(&radius.to_le_bytes());
        hasher.update(&nonce.to_le_bytes());
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    use uuid::Uuid;

    fn peer_from(i: u128) -> matchbox_socket::PeerId {
        matchbox_socket::PeerId(Uuid::from_u128(i + 1))
    }

    fn known_peers(count: usize) -> Vec<matchbox_socket::PeerId> {
        (0..count as u128).map(peer_from).collect()
    }

    #[test]
    fn role_link_caps_are_enforced() {
        let peers = known_peers(64);
        let local = peers[0];
        let parent = peers[1];
        let backup = peers[2];

        let mut leaf = NetworkSession::new();
        leaf.local_peer_id = Some(local);
        leaf.is_host = false;
        leaf.supernode_set = vec![peers[5], peers[6], peers[7]];
        leaf.relay_parent = Some(parent);
        leaf.relay_backup_parent = Some(backup);
        leaf.relay_children = Vec::new();
        let leaf_links = leaf.desired_peer_links(&peers, &peers);
        assert!(leaf_links.contains(&parent));
        assert!(leaf_links.contains(&backup));
        assert!(leaf_links.len() <= NetworkSession::LEAF_LINK_CAP);

        let mut supernode = NetworkSession::new();
        supernode.local_peer_id = Some(local);
        supernode.is_host = false;
        supernode.supernode_set = vec![local, peers[5], peers[6], peers[7]];
        supernode.relay_parent = Some(parent);
        supernode.relay_backup_parent = Some(backup);
        supernode.relay_children = peers[8..16].to_vec();
        let super_links = supernode.desired_peer_links(&peers, &peers);
        assert!(super_links.contains(&parent));
        assert!(super_links.contains(&backup));
        for child in &supernode.relay_children {
            assert!(super_links.contains(child));
        }
        assert!(super_links.len() <= NetworkSession::SUPERNODE_LINK_CAP);

        let mut root = NetworkSession::new();
        root.local_peer_id = Some(local);
        root.is_host = true;
        root.relay_children = peers[1..13].to_vec();
        let root_links = root.desired_peer_links(&peers, &peers);
        for child in &root.relay_children {
            assert!(root_links.contains(child));
        }
        assert!(root_links.len() <= NetworkSession::ROOT_LINK_CAP);
    }

    #[test]
    fn profile_sparse_link_selection() {
        let sizes = [1usize, 2, 3, 7, 12, 21, 32, 100, 1_000, 10_000, 100_000];

        for size in sizes {
            let peers = known_peers(size.max(1));
            let local = peers[0];
            let fanout = NetworkSession::choose_dynamic_fanout(size.max(1));
            let child_count = fanout.min(peers.len().saturating_sub(1));

            let mut root = NetworkSession::new();
            root.local_peer_id = Some(local);
            root.is_host = true;
            root.relay_fanout = fanout;
            root.relay_children = peers[1..(1 + child_count)].to_vec();

            let loops = if size >= 10_000 { 20 } else if size >= 1_000 { 60 } else { 200 };
            let start = Instant::now();
            let mut max_links = 0usize;
            for i in 0..loops {
                root.relay_epoch = i as u32;
                let desired = root.desired_peer_links(&peers, &peers);
                max_links = max_links.max(desired.len());
            }
            let elapsed = start.elapsed();
            println!(
                "size={} fanout={} loops={} max_links={} elapsed_ms={}",
                size,
                fanout,
                loops,
                max_links,
                elapsed.as_millis()
            );
            assert!(max_links <= NetworkSession::ROOT_LINK_CAP);
        }
    }
}

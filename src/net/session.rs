use crate::math::Vec2;
use crate::net::{
    AreaAuthorityEntry, AreaAuthorityUpdate, CannonShot, ChatMessage, EnemyDamage, EnemyKill,
    EnemyKillBatch, EnemySync, InputFrame, InputFrameBatch, InputFrameEntry, JoinRequest,
    NetMessage, PaidAbility, PaidAbilityAck, PaidAbilityType, PaidNameAck, PaidNameReservation,
    PaidNameSync, PaidObstacle, PaidObstacleAck, PaidObstacleSync, Ping, PlayerDeath, PlayerState,
    PlayerStateBatch, PlayerStateEntry, PlayerStatsSnapshot, Pong, ProjectileReflection,
    RemotePlayer, SupernodeScore, TopologyDelta, TopologyEntry, TopologyUpdate, VoteMute,
    WaveStart,
};
use crate::world::CHUNK_SIZE;
use js_sys;
use matchbox_socket::{PeerState, WebRtcSocket};
use sha2::{Digest, Sha256};
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

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
            // Free public STUN from two independent operators, queried in
            // parallel; one being unreachable costs nothing (gathering is
            // capped at 3s in the socket fork). STUN is a one-shot, stateless
            // "what is my public address?" echo at link setup — it never
            // carries game data and any server is interchangeable.
            urls: vec![
                "stun:stun.l.google.com:19302".to_string(),
                "stun:stun1.l.google.com:19302".to_string(),
                "stun:stun.cloudflare.com:3478".to_string(),
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

#[derive(Debug, Clone)]
pub struct PendingEnemySync {
    pub origin_hash: u64,
    pub from_host: bool,
    pub introductions_only: bool,
    pub sync: EnemySync,
}

#[derive(Debug, Clone, Copy)]
enum LowPriorityTopic {
    Chat,
    Vote,
    Ack,
    Stats,
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
    local_name_owner_hash: u64,
    pub supernode_id: Option<matchbox_socket::PeerId>,
    pub super_root_id: Option<matchbox_socket::PeerId>,
    pub supernode_set: Vec<matchbox_socket::PeerId>,
    relay_parent: Option<matchbox_socket::PeerId>,
    relay_backup_parent: Option<matchbox_socket::PeerId>,
    relay_active_parent: Option<matchbox_socket::PeerId>,
    relay_fanout: usize,
    relay_children: Vec<matchbox_socket::PeerId>,
    desired_peer_set: HashSet<matchbox_socket::PeerId>,
    discovery_attached: bool,
    /// Last applied room-wide relay map (authoritative copy on the root).
    topology_roster: Vec<TopologyEntry>,
    /// Cached `{:?}` strings of roster uuids so per-frame lookup pruning keeps
    /// hash->id mappings alive for roster members we have not connected to yet.
    roster_peer_strs: HashSet<String>,
    /// Root only: last frame each member (by origin hash) showed signs of life.
    member_last_seen: HashMap<u64, u32>,
    /// Root only: membership/assignments changed since last broadcast.
    roster_dirty: bool,
    last_map_rx_frame: u32,
    last_join_request_frame: u32,
    /// Peers that asked to join through us; they get the next map directly.
    pending_map_peers: Vec<matchbox_socket::PeerId>,
    /// Join requests forwarded recently (origin hash -> frame), to dedupe relays.
    recent_join_forwards: HashMap<u64, u32>,
    /// Connected links no longer in the desired set (peer -> frame it became
    /// undesired). Dropped only after a grace window: make-before-break.
    undesired_since: HashMap<matchbox_socket::PeerId, u32>,
    /// First frame at which we had any connected peer (bootstrap promotion timer).
    first_peer_frame: Option<u32>,
    flush_counter: u32,
    last_name_announce_frame: u32,
    /// Frame at which we got positive evidence the root is gone (signaling
    /// departure broadcast, or its direct link dying on its own). Triggers the
    /// fast failover path instead of waiting out the silence timeout.
    root_departure_frame: Option<u32>,
    /// Links we closed ourselves (grace trims); their Disconnected events are
    /// expected and must not count as death evidence. Frame-stamped for expiry.
    intentional_drops: HashMap<matchbox_socket::PeerId, u32>,
    /// Root only: roster as of the last broadcast, for delta computation.
    last_broadcast_roster: Vec<TopologyEntry>,
    last_broadcast_epoch: u32,
    map_anchor_counter: u32,
    /// Receiver fell out of sync with the delta stream; ask for a full map.
    needs_full_map: bool,
    /// Test/debug override for tree fanout (forces deep topologies with few
    /// nodes, e.g. fanout 1 turns a 3-player room into a chain).
    fanout_override: Option<usize>,
    /// Frame at which we last acquired the root role (handoff hysteresis:
    /// a fresh root holds the role for a settle period before it may hand
    /// off again, so alt-tabbing players don't ping-pong the role).
    root_acquired_frame: u32,
    /// Root only: when each member's current parent was assigned.
    member_parent_since: HashMap<u64, u32>,
    /// Root only: repeated join requests from an in-roster member signal the
    /// assigned parent link never formed (e.g. hard-NAT pair): (count, frame).
    member_link_nags: HashMap<u64, (u32, u32)>,
    /// Reconnect parameters for auto-rejoin after signaling/socket loss.
    reconnect_signaling: Option<String>,
    reconnect_ice: Option<IceConfig>,
    auto_rejoin_enabled: bool,
    rejoin_attempts: u32,
    next_rejoin_frame: u32,
    relay_epoch: u32,
    last_parent_switch_frame: u32,
    stale_parent_events: u32,
    peer_link_backoff_until: HashMap<matchbox_socket::PeerId, u32>,
    last_peer_message_frames: HashMap<matchbox_socket::PeerId, u32>,
    peer_connected_frames: HashMap<matchbox_socket::PeerId, u32>,
    pub relay_telemetry: RelayTelemetry,
    last_update_frame: u32,
    last_lowpri_chat_frame: u32,
    last_lowpri_vote_frame: u32,
    last_lowpri_ack_frame: u32,
    last_lowpri_stats_frame: u32,
    last_telemetry_log_frame: u32,
    last_sync_trace_frame: u32,
    last_sync_trace_sig: u64,
    last_sync_warn_frame: u32,
    area_authorities: HashMap<u32, u64>,
    last_topology_broadcast_frame: u32,
    last_area_update_broadcast_frame: u32,
    local_last_pos: Option<Vec2>,
    pub local_stats: PlayerStats,
    pub remote_stats: HashMap<PeerId, PlayerStats>,
    pending_player_names: HashMap<PeerId, String>,
    peer_id_lookup: HashMap<PeerId, matchbox_socket::PeerId>,
    peer_hash_lookup: HashMap<u64, PeerId>,
    peer_identity_lookup: HashMap<u64, PeerId>,
    pending_messages: Vec<(PeerId, Option<u64>, NetMessage)>,
    /// Whether this client is the host (room creator) - host controls enemy spawning
    /// Relay-tree root (lowest sorted [`PeerId`] among known/connected nodes). Drives topology
    /// broadcast and `send_*` routing—not necessarily the player who clicked "create room".
    /// Game simulation authority (waves/enemy sync) currently follows this same flag.
    pub is_host: bool,
    /// Received enemy-sync corrections. Multiple per frame are normal once
    /// per-area authorities are active.
    pub pending_enemy_syncs: Vec<PendingEnemySync>,
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
    /// Received paid username reservation events
    pub pending_paid_names: Vec<(PeerId, PaidNameReservation)>,
    /// Received paid username reservation verification acks
    pub pending_paid_name_acks: Vec<(PeerId, PaidNameAck)>,
    pending_paid_name_candidates: HashMap<[u8; 32], PaidNameReservation>,
    /// Received cannon shot events from host
    pub pending_cannon_shots: Vec<CannonShot>,
    /// Received projectile reflection events from peers
    pub pending_projectile_reflections: Vec<ProjectileReflection>,
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
    last_enemy_sync_frame: u32,
    paid_obstacle_confirmations: HashMap<[u8; 32], HashSet<matchbox_socket::PeerId>>,
    paid_ability_confirmations: HashMap<[u8; 32], HashSet<matchbox_socket::PeerId>>,
    paid_name_confirmations: HashMap<[u8; 32], HashSet<matchbox_socket::PeerId>>,
    name_reservations: HashMap<String, PaidNameReservation>,
    last_ping_frame: u32,
    last_score_frame: u32,
    /// Newly connected peers that need current game state (for late joiners)
    pub new_peers_needing_state: Vec<matchbox_socket::PeerId>,
}

impl NetworkSession {
    const MAX_FANOUT: usize = 16;
    const MIN_FANOUT: usize = 2;
    const PEER_HANDSHAKE_GRACE_FRAMES: u32 = 240;
    const AREA_GROUP_CHUNKS: i32 = 4;
    const RELAY_PARENT_STALE_FRAMES: u32 = 180;
    const RELAY_FAILOVER_COOLDOWN_FRAMES: u32 = 120;
    const RELAY_FAILOVER_MIN_SAMPLES: u8 = 2;
    const RELAY_HANDOFF_DUPLEX_FRAMES: u32 = 24;
    const LEAF_LINK_CAP: usize = 7;
    const SUPERNODE_LINK_CAP: usize = 24;
    const ROOT_LINK_CAP: usize = 28;
    const SYNC_TRACE_PERIOD_FRAMES: u32 = 60;
    const SYNC_STALE_WARN_FRAMES: u32 = 180;
    const STALE_LINK_RESET_FRAMES: u32 = 240;
    const STALE_LINK_BACKOFF_FRAMES: u32 = 120;
    const MAX_DISCOVERY_PEERS: usize = 192;
    const MAX_RELAY_INPUT_QUEUE: usize = 1024;
    const MAX_DOWNLINK_QUEUE: usize = 64;
    const MAX_BATCH_ENTRIES: usize = 512;
    const RELAY_ENVELOPE_MAGIC: [u8; 4] = *b"SLRY";
    /// Root drops a member that has shown no signs of life for this long (~10s).
    const MEMBER_TTL_FRAMES: u32 = 600;
    /// Root re-broadcasts the (possibly unchanged) map at this cadence; the
    /// periodic flood down the tree doubles as the root liveness heartbeat.
    const MAP_BROADCAST_PERIOD_FRAMES: u32 = 60;
    /// How often a node without a tree slot re-sends its JoinRequest.
    const JOIN_REQUEST_PERIOD_FRAMES: u32 = 90;
    /// No map for this long (~30s) means the root is presumed dead.
    const ROOT_STALE_FRAMES: u32 = 1800;
    /// Successor candidates take over in uuid order, staggered by this much,
    /// so the room converges on one new root without extra coordination.
    const ROOT_TAKEOVER_STAGGER_FRAMES: u32 = 300;
    /// With positive death evidence (signaling departure / link death) the
    /// takeover is fast: ~3s for the first candidate, +2s per rank.
    const ROOT_DEPARTURE_TAKEOVER_BASE_FRAMES: u32 = 180;
    const ROOT_DEPARTURE_TAKEOVER_STAGGER_FRAMES: u32 = 120;
    /// Never received any map after this long with live links: assume the room
    /// has no root (creator left) and let the lowest-uuid connected node claim it.
    const BOOTSTRAP_NO_ROOT_FRAMES: u32 = 900;
    /// Healthy-but-undesired links are kept this long before being dropped
    /// (make-before-break: replacement tree links form while old ones serve).
    const UNDESIRED_LINK_GRACE_FRAMES: u32 = 600;
    /// How long a forwarded JoinRequest suppresses re-forwarding for the same origin.
    const JOIN_FORWARD_DEDUP_FRAMES: u32 = 60;
    /// Rooms above this size broadcast topology deltas/heartbeats instead of
    /// full maps (small rooms keep the simpler battle-tested full-map path).
    const DELTA_ROSTER_MIN: usize = 33;
    /// In delta mode, every Nth broadcast is a full-map anchor.
    const MAP_FULL_ANCHOR_EVERY: u32 = 10;
    /// Coalesce membership-change broadcasts in big rooms (join replies still
    /// go out immediately via pending_map_peers).
    const MIN_DIRTY_BROADCAST_FRAMES: u32 = 30;
    /// A member must hold a parent this long before connectivity nags can
    /// trigger reassignment, and reassignments are at most this frequent.
    const REASSIGN_MIN_PARENT_AGE_FRAMES: u32 = 600;
    /// Join-request nags from an in-roster member count within this window...
    const LINK_NAG_WINDOW_FRAMES: u32 = 300;
    /// ...and this many of them mean "my assigned parent is unreachable".
    const LINK_NAG_THRESHOLD: u32 = 2;
    /// First auto-rejoin attempt happens this long after socket loss.
    const REJOIN_BASE_DELAY_FRAMES: u32 = 60;

    fn interest_radius_sq() -> f32 {
        let radius = 1800.0;
        radius * radius
    }

    pub fn area_id_for_pos(pos: Vec2) -> u32 {
        Self::area_id_from_pos(pos)
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

    fn identity_peer_id_for_hash(&self, hash: u64) -> Option<PeerId> {
        self.peer_identity_lookup
            .get(&hash)
            .cloned()
            .or_else(|| self.peer_hash_lookup.get(&hash).cloned())
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
        if let Some(peer_id) = self.identity_peer_id_for_hash(peer_hash) {
            if let Some(remote) = self.remote_players.get(&peer_id) {
                return Self::area_id_from_pos(remote.pos);
            }
        }
        0
    }

    fn peer_pos(&self, peer_id: matchbox_socket::PeerId) -> Option<Vec2> {
        if Some(peer_id) == self.local_peer_id {
            return self.local_last_pos;
        }
        let peer_hash = Self::peer_hash_for_matchbox(peer_id);
        let peer_key = self
            .identity_peer_id_for_hash(peer_hash)
            .unwrap_or_else(|| format!("{:?}", peer_id));
        self.remote_players.get(&peer_key).map(|remote| remote.pos)
    }

    fn choose_dynamic_fanout(total_nodes: usize) -> usize {
        let fanout = match total_nodes {
            0..=3 => 2,
            4..=8 => 4,
            9..=16 => 5,
            17..=32 => 6,
            33..=64 => 7,
            65..=128 => 8,
            129..=256 => 9,
            257..=512 => 10,
            513..=2048 => 12,
            2049..=8192 => 14,
            _ => 16,
        };
        fanout.clamp(Self::MIN_FANOUT, Self::MAX_FANOUT)
    }

    /// Console logging that is a no-op in native unit tests.
    fn log_info(message: &str) {
        #[cfg(target_arch = "wasm32")]
        web_sys::console::log_1(&message.into());
        #[cfg(not(target_arch = "wasm32"))]
        let _ = message;
    }

    fn log_warn(message: &str) {
        #[cfg(target_arch = "wasm32")]
        web_sys::console::warn_1(&message.into());
        #[cfg(not(target_arch = "wasm32"))]
        let _ = message;
    }

    fn peer_id_from_uuid(uuid: [u8; 16]) -> matchbox_socket::PeerId {
        matchbox_socket::PeerId(uuid::Uuid::from_bytes(uuid))
    }

    fn uuid_for_peer(peer_id: matchbox_socket::PeerId) -> [u8; 16] {
        *peer_id.0.as_bytes()
    }

    /// Make a roster member resolvable: hash -> `{:?}` string -> matchbox PeerId,
    /// even if we have never exchanged a message with it.
    fn register_peer_uuid(&mut self, uuid: [u8; 16]) -> (u64, matchbox_socket::PeerId) {
        let peer = Self::peer_id_from_uuid(uuid);
        let peer_str = format!("{:?}", peer);
        let hash = Self::hash_peer_id(&peer_str);
        self.peer_id_lookup.insert(peer_str.clone(), peer);
        self.peer_hash_lookup.insert(hash, peer_str.clone());
        self.peer_identity_lookup
            .entry(hash)
            .or_insert_with(|| peer_str.clone());
        (hash, peer)
    }

    fn roster_entry(&self, peer_hash: u64) -> Option<&TopologyEntry> {
        self.topology_roster
            .iter()
            .find(|entry| entry.peer_hash == peer_hash)
    }

    fn roster_contains(&self, peer_hash: u64) -> bool {
        self.roster_entry(peer_hash).is_some()
    }

    fn roster_root_hash(&self) -> Option<u64> {
        self.topology_roster
            .iter()
            .find(|entry| entry.parent_hash == 0)
            .map(|entry| entry.peer_hash)
    }

    fn roster_child_count(&self, peer_hash: u64) -> usize {
        self.topology_roster
            .iter()
            .filter(|entry| entry.parent_hash == peer_hash)
            .count()
    }

    fn rebuild_roster_peer_strs(&mut self) {
        self.roster_peer_strs = self
            .topology_roster
            .iter()
            .map(|entry| format!("{:?}", Self::peer_id_from_uuid(entry.uuid)))
            .collect();
    }

    /// Root: any sign of life from a member refreshes its TTL.
    fn note_member_seen(&mut self, origin_hash: u64, current_frame: u32) {
        if self.is_host {
            self.member_last_seen.insert(origin_hash, current_frame);
        }
    }

    /// Root: sticky parent assignment. New members attach to the earliest
    /// roster member (root first, then join order) with spare fanout, so a
    /// join never reshuffles existing routes.
    fn pick_parent_for_new_member(&self) -> u64 {
        let fanout = self.relay_fanout.clamp(1, Self::MAX_FANOUT);
        for entry in &self.topology_roster {
            if self.roster_child_count(entry.peer_hash) < fanout {
                return entry.peer_hash;
            }
        }
        self.roster_root_hash().unwrap_or(0)
    }

    /// Root: add a member to the roster (idempotent).
    fn add_roster_member(&mut self, uuid: [u8; 16], current_frame: u32) {
        let (hash, _) = self.register_peer_uuid(uuid);
        self.member_last_seen.insert(hash, current_frame);
        if Some(hash) == self.local_peer_hash || self.roster_contains(hash) {
            return;
        }
        let parent_hash = self.pick_parent_for_new_member();
        self.topology_roster.push(TopologyEntry {
            peer_hash: hash,
            uuid,
            parent_hash,
        });
        self.member_parent_since.insert(hash, current_frame);
        self.roster_dirty = true;
    }

    /// Order-independent fingerprint of a roster's structure. Lets delta
    /// receivers prove they reconstructed exactly what the root has.
    fn roster_checksum(entries: &[TopologyEntry]) -> u64 {
        let mut pairs: Vec<(u64, u64)> = entries
            .iter()
            .map(|entry| (entry.peer_hash, entry.parent_hash))
            .collect();
        pairs.sort_unstable();
        let mut hash: u64 = 0xcbf29ce484222325;
        for (peer, parent) in pairs {
            for byte in peer.to_le_bytes().iter().chain(parent.to_le_bytes().iter()) {
                hash ^= *byte as u64;
                hash = hash.wrapping_mul(0x100000001b3);
            }
        }
        hash
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
            local_name_owner_hash: 0,
            supernode_id: None,
            super_root_id: None,
            supernode_set: Vec::new(),
            relay_parent: None,
            relay_backup_parent: None,
            relay_active_parent: None,
            relay_fanout: Self::MIN_FANOUT,
            relay_children: Vec::new(),
            desired_peer_set: HashSet::new(),
            discovery_attached: false,
            topology_roster: Vec::new(),
            roster_peer_strs: HashSet::new(),
            member_last_seen: HashMap::new(),
            roster_dirty: false,
            last_map_rx_frame: 0,
            last_join_request_frame: 0,
            pending_map_peers: Vec::new(),
            recent_join_forwards: HashMap::new(),
            undesired_since: HashMap::new(),
            first_peer_frame: None,
            flush_counter: 0,
            last_name_announce_frame: 0,
            root_departure_frame: None,
            intentional_drops: HashMap::new(),
            last_broadcast_roster: Vec::new(),
            last_broadcast_epoch: 0,
            map_anchor_counter: 0,
            needs_full_map: false,
            fanout_override: None,
            root_acquired_frame: 0,
            member_parent_since: HashMap::new(),
            member_link_nags: HashMap::new(),
            reconnect_signaling: None,
            reconnect_ice: None,
            auto_rejoin_enabled: false,
            rejoin_attempts: 0,
            next_rejoin_frame: 0,
            relay_epoch: 0,
            last_parent_switch_frame: 0,
            stale_parent_events: 0,
            peer_link_backoff_until: HashMap::new(),
            last_peer_message_frames: HashMap::new(),
            peer_connected_frames: HashMap::new(),
            relay_telemetry: RelayTelemetry::default(),
            last_update_frame: 0,
            last_lowpri_chat_frame: u32::MAX,
            last_lowpri_vote_frame: u32::MAX,
            last_lowpri_ack_frame: u32::MAX,
            last_lowpri_stats_frame: u32::MAX,
            last_telemetry_log_frame: 0,
            last_sync_trace_frame: 0,
            last_sync_trace_sig: 0,
            last_sync_warn_frame: 0,
            area_authorities: HashMap::new(),
            last_topology_broadcast_frame: 0,
            last_area_update_broadcast_frame: 0,
            local_last_pos: None,
            local_stats: PlayerStats::default(),
            remote_stats: HashMap::new(),
            pending_player_names: HashMap::new(),
            peer_id_lookup: HashMap::new(),
            peer_hash_lookup: HashMap::new(),
            peer_identity_lookup: HashMap::new(),
            pending_messages: Vec::new(),
            is_host: false,
            pending_enemy_syncs: Vec::new(),
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
            pending_paid_names: Vec::new(),
            pending_paid_name_acks: Vec::new(),
            pending_paid_name_candidates: HashMap::new(),
            pending_cannon_shots: Vec::new(),
            pending_projectile_reflections: Vec::new(),
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
            last_enemy_sync_frame: 0,
            paid_obstacle_confirmations: HashMap::new(),
            paid_ability_confirmations: HashMap::new(),
            paid_name_confirmations: HashMap::new(),
            name_reservations: HashMap::new(),
            last_ping_frame: 0,
            last_score_frame: 0,
            new_peers_needing_state: Vec::new(),
        }
    }

    /// Generate a random default player name
    fn generate_default_name() -> String {
        use rand::Rng;
        let adjectives = [
            "Swift", "Brave", "Sly", "Bold", "Keen", "Wild", "Cool", "Rad",
        ];
        let nouns = [
            "Slime", "Blob", "Goo", "Ooze", "Jelly", "Glob", "Puddle", "Drop",
        ];
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
        (0..6)
            .map(|_| chars[rng.gen_range(0..chars.len())])
            .collect()
    }

    /// Set the local player's name
    pub fn set_player_name(&mut self, name: &str) {
        self.local_player_name = if name.is_empty() {
            Self::generate_default_name()
        } else {
            Self::normalize_player_name(name)
        };
    }

    pub fn hash_name_owner_seed(seed: &str) -> u64 {
        let hash = Self::hash_peer_id(seed);
        if hash == 0 {
            1
        } else {
            hash
        }
    }

    pub fn set_local_name_owner_hash(&mut self, owner_hash: u64) {
        self.local_name_owner_hash = owner_hash.max(1);
    }

    pub fn local_name_owner_hash(&self) -> u64 {
        if self.local_name_owner_hash != 0 {
            self.local_name_owner_hash
        } else {
            self.local_peer_hash.unwrap_or(0)
        }
    }

    pub fn broadcast_local_player_name(&mut self) {
        let socket = match &mut self.socket {
            Some(s) => s,
            None => return,
        };
        let msg = NetMessage::PlayerJoined(self.local_player_name.clone()).to_bytes();
        let peers: Vec<_> = socket.connected_peers().collect();
        for peer_id in peers {
            socket.send(msg.clone().into_boxed_slice(), peer_id);
        }
    }

    /// Create a new room and return the room code
    /// The creator becomes the host and controls enemy spawning
    pub fn create_room(&mut self, signaling_server: &str, ice_config: &IceConfig) -> String {
        self.room_code = Self::generate_room_code();
        self.is_host = true; // Room creator is the host
        self.auto_rejoin_enabled = true;
        self.rejoin_attempts = 0;
        self.next_rejoin_frame = 0;
        let room_code = self.room_code.clone();
        self.connect(signaling_server, &room_code, ice_config);
        self.room_code.clone()
    }

    /// Join an existing room by code
    pub fn join_room(&mut self, signaling_server: &str, room_code: &str, ice_config: &IceConfig) {
        self.room_code = room_code.to_uppercase();
        self.is_host = false; // Joiners are not hosts
        self.auto_rejoin_enabled = true;
        self.rejoin_attempts = 0;
        self.next_rejoin_frame = 0;
        let room_code = self.room_code.clone();
        self.connect(signaling_server, &room_code, ice_config);
    }

    fn connect(&mut self, signaling_server: &str, room_code: &str, ice_config: &IceConfig) {
        // Game-specific room path. Omit `?next=N` so Matchbox keeps the full room membership
        // (pairwise `next=2` is for 2-player sequential pairing, not N-peer free rooms).
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
        let (socket, loop_fut) = WebRtcSocket::builder(&room_url)
            .ice_server(ice_server)
            .add_reliable_channel()
            .build();
        // Start in default full-mesh signaling mode so first-handshake offers are not
        // filtered before we know any peers. We tighten to sparse desired links once
        // the root's topology map arrives. All nodes stay attached to signaling for
        // the lifetime of the session: it is the membership oracle and the only way
        // to form new WebRTC links (no overlay-relayed signaling yet).
        self.socket = Some(socket);
        self.desired_peer_set.clear();
        self.discovery_attached = true;
        self.topology_roster.clear();
        self.roster_peer_strs.clear();
        self.member_last_seen.clear();
        self.roster_dirty = false;
        self.last_map_rx_frame = 0;
        self.last_join_request_frame = 0;
        self.pending_map_peers.clear();
        self.recent_join_forwards.clear();
        self.undesired_since.clear();
        self.first_peer_frame = None;
        self.root_departure_frame = None;
        self.intentional_drops.clear();
        // A (re)connect means a fresh identity and a fresh overlay: stale
        // relay routes from a previous socket would poison the desired set
        // (e.g. desiring a dead room's root forever) and block admission.
        self.super_root_id = None;
        self.supernode_id = None;
        self.supernode_set.clear();
        self.relay_parent = None;
        self.relay_backup_parent = None;
        self.relay_active_parent = None;
        self.relay_children.clear();
        self.relay_fanout = Self::MIN_FANOUT;
        self.relay_epoch = 0;
        self.area_authorities.clear();
        self.peer_link_backoff_until.clear();
        self.last_peer_message_frames.clear();
        self.peer_connected_frames.clear();
        self.latency_ms.clear();
        self.latency_samples.clear();
        self.supernode_scores.clear();
        self.last_topology_broadcast_frame = 0;
        self.last_broadcast_roster.clear();
        self.last_broadcast_epoch = 0;
        self.map_anchor_counter = 0;
        self.needs_full_map = false;
        self.member_parent_since.clear();
        self.member_link_nags.clear();
        self.reconnect_signaling = Some(signaling_server.to_string());
        self.reconnect_ice = Some(ice_config.clone());
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
        self.relay_parent = None;
        self.relay_backup_parent = None;
        self.relay_active_parent = None;
        self.relay_fanout = Self::MIN_FANOUT;
        self.relay_children.clear();
        self.desired_peer_set.clear();
        self.discovery_attached = false;
        self.topology_roster.clear();
        self.roster_peer_strs.clear();
        self.member_last_seen.clear();
        self.roster_dirty = false;
        self.last_map_rx_frame = 0;
        self.last_join_request_frame = 0;
        self.pending_map_peers.clear();
        self.recent_join_forwards.clear();
        self.undesired_since.clear();
        self.first_peer_frame = None;
        self.flush_counter = 0;
        self.last_name_announce_frame = 0;
        self.root_departure_frame = None;
        self.intentional_drops.clear();
        self.last_broadcast_roster.clear();
        self.last_broadcast_epoch = 0;
        self.map_anchor_counter = 0;
        self.needs_full_map = false;
        self.member_parent_since.clear();
        self.member_link_nags.clear();
        self.reconnect_signaling = None;
        self.reconnect_ice = None;
        self.auto_rejoin_enabled = false;
        self.rejoin_attempts = 0;
        self.next_rejoin_frame = 0;
        self.relay_epoch = 0;
        self.last_parent_switch_frame = 0;
        self.stale_parent_events = 0;
        self.peer_link_backoff_until.clear();
        self.last_peer_message_frames.clear();
        self.peer_connected_frames.clear();
        self.relay_telemetry = RelayTelemetry::default();
        self.last_update_frame = 0;
        self.last_lowpri_chat_frame = u32::MAX;
        self.last_lowpri_vote_frame = u32::MAX;
        self.last_lowpri_ack_frame = u32::MAX;
        self.last_lowpri_stats_frame = u32::MAX;
        self.last_telemetry_log_frame = 0;
        self.last_sync_trace_frame = 0;
        self.last_sync_trace_sig = 0;
        self.last_sync_warn_frame = 0;
        self.area_authorities.clear();
        self.local_last_pos = None;
        self.pending_player_names.clear();
        self.peer_id_lookup.clear();
        self.peer_hash_lookup.clear();
        self.peer_identity_lookup.clear();
        self.last_enemy_sync_frame = 0;
        self.paid_obstacle_confirmations.clear();
        self.paid_ability_confirmations.clear();
        self.paid_name_confirmations.clear();
        self.pending_paid_obstacles.clear();
        self.pending_paid_obstacle_acks.clear();
        self.pending_paid_abilities.clear();
        self.pending_paid_ability_acks.clear();
        self.pending_paid_names.clear();
        self.pending_paid_name_acks.clear();
        self.pending_paid_name_candidates.clear();
        self.pending_cannon_shots.clear();
        self.pending_enemy_syncs.clear();
        self.pending_projectile_reflections.clear();
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
            self.socket = None;
            // Signaling/socket loss is recoverable: rejoin the same room with
            // backoff instead of dumping the player to the title screen. We
            // come back under a fresh peer id; the join machinery re-admits
            // us and the old identity ages out of rosters.
            if self.auto_rejoin_enabled
                && !self.room_code.is_empty()
                && self.reconnect_signaling.is_some()
            {
                if self.next_rejoin_frame == 0 {
                    Self::log_warn("Connection lost; will auto-rejoin");
                    self.next_rejoin_frame =
                        current_frame.saturating_add(Self::REJOIN_BASE_DELAY_FRAMES);
                }
                self.state = NetworkState::WaitingForPeers;
                if current_frame >= self.next_rejoin_frame {
                    self.rejoin_attempts = self.rejoin_attempts.saturating_add(1);
                    let backoff = Self::REJOIN_BASE_DELAY_FRAMES
                        .saturating_mul(1u32 << self.rejoin_attempts.min(4))
                        .min(900);
                    self.next_rejoin_frame = current_frame.saturating_add(backoff);
                    let server = self.reconnect_signaling.clone().unwrap_or_default();
                    let ice = self.reconnect_ice.clone().unwrap_or_default();
                    let room = self.room_code.clone();
                    // Solo creators keep root so their room re-forms instantly;
                    // with others around, rejoin as a regular member (the
                    // survivors fail over on their own).
                    let retain_host = self.is_host && self.remote_players.is_empty();
                    self.local_peer_id = None;
                    self.local_peer_hash = None;
                    Self::log_warn(&format!(
                        "Auto-rejoining room {room} (attempt {})",
                        self.rejoin_attempts
                    ));
                    self.connect(&server, &room, &ice);
                    self.is_host = retain_host;
                }
                return true;
            }
            Self::log_info("Network connection closed");
            self.state = NetworkState::Error("Connection failed".to_string());
            return false;
        }

        let (local_id, connected_peers, known_peers, departures) = {
            let socket = match &mut self.socket {
                Some(s) => s,
                None => return true, // No socket is fine, just nothing to update
            };

            // Try to update peers - this is safe now because we check socket_closed first
            let peers = socket.update_peers();

            // Check for new peers
            let local_name = self.local_player_name.clone();
            let seen_message_frames = self.last_peer_message_frames.clone();
            let connected_frames = self.peer_connected_frames.clone();
            for (peer_id, peer_state) in peers {
                let peer_id_str = format!("{:?}", peer_id);
                match peer_state {
                    PeerState::Connected => {
                        web_sys::console::log_1(&format!("Peer connected: {}", peer_id_str).into());
                        self.state = NetworkState::Connected;
                        self.peer_id_lookup.insert(peer_id_str.clone(), peer_id);
                        self.peer_connected_frames.insert(peer_id, current_frame);
                        self.peer_link_backoff_until.remove(&peer_id);
                        let hash = Self::hash_peer_id(&peer_id_str);
                        self.peer_hash_lookup.insert(hash, peer_id_str.clone());
                        self.peer_identity_lookup
                            .entry(hash)
                            .or_insert_with(|| peer_id_str.clone());
                        // Send join message with our name
                        let msg = NetMessage::PlayerJoined(local_name.clone()).to_bytes();
                        socket.send(msg.into_boxed_slice(), peer_id);
                        // Track new peer so the elected supernode can send state
                        self.new_peers_needing_state.push(peer_id);
                    }
                    PeerState::Disconnected => {
                        let last_seen_age = seen_message_frames
                            .get(&peer_id)
                            .map(|frame| current_frame.saturating_sub(*frame));
                        let connected_age = connected_frames
                            .get(&peer_id)
                            .map(|frame| current_frame.saturating_sub(*frame));
                        let seen_age_label = last_seen_age
                            .map(|age| age.to_string())
                            .unwrap_or_else(|| "none".to_string());
                        let connected_age_label = connected_age
                            .map(|age| age.to_string())
                            .unwrap_or_else(|| "none".to_string());
                        web_sys::console::log_1(
                            &format!("Peer disconnected: {}", peer_id_str).into(),
                        );
                        web_sys::console::warn_1(
                            &format!(
                                "[sync-trace f={current_frame}] disconnect peer={peer_id_str} seen_age={} connected_age={} conn_before={} desired={} discovery={} epoch={} parent={:?} active={:?} backup={:?} root={:?} super={:?}",
                                seen_age_label,
                                connected_age_label,
                                self.peer_id_lookup.len(),
                                self.desired_peer_set.len(),
                                self.discovery_attached,
                                self.relay_epoch,
                                self.relay_parent,
                                self.relay_active_parent,
                                self.relay_backup_parent,
                                self.super_root_id,
                                self.supernode_id,
                            )
                            .into(),
                        );
                        let hash = Self::hash_peer_id(&peer_id_str);
                        let canonical_peer_id = self
                            .peer_identity_lookup
                            .get(&hash)
                            .cloned()
                            .unwrap_or_else(|| peer_id_str.clone());
                        if canonical_peer_id == peer_id_str {
                            self.remote_players.remove(&canonical_peer_id);
                            self.remote_stats.remove(&canonical_peer_id);
                            self.pending_player_names.remove(&canonical_peer_id);
                            self.peer_identity_lookup.remove(&hash);
                        }
                        if let Some(peer_id) = self.peer_id_lookup.get(&peer_id_str).copied() {
                            self.supernode_scores.remove(&peer_id);
                            self.latency_ms.remove(&peer_id);
                            self.latency_samples.remove(&peer_id);
                            self.peer_link_backoff_until.remove(&peer_id);
                            self.last_peer_message_frames.remove(&peer_id);
                            self.peer_connected_frames.remove(&peer_id);
                            let expected = self.intentional_drops.remove(&peer_id).is_some();
                            if !expected
                                && !self.is_host
                                && Some(peer_id) == self.super_root_id
                                && self.root_departure_frame.is_none()
                            {
                                self.root_departure_frame = Some(current_frame);
                            }
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
                self.peer_identity_lookup
                    .entry(hash)
                    .or_insert_with(|| peer_id_str.clone());
                self.last_peer_message_frames.insert(peer_id, current_frame);
                self.peer_link_backoff_until.remove(&peer_id);
                self.relay_telemetry.recv_messages =
                    self.relay_telemetry.recv_messages.saturating_add(1);
                if self.is_host {
                    self.member_last_seen.insert(hash, current_frame);
                }
                if let Some((origin_hash, payload)) = Self::decode_relay_envelope(&data) {
                    if self.is_host {
                        self.member_last_seen.insert(origin_hash, current_frame);
                    }
                    if let Some(msg) = NetMessage::from_bytes(&payload) {
                        self.pending_messages
                            .push((peer_id_str, Some(origin_hash), msg));
                    } else {
                        self.relay_telemetry.dropped_messages =
                            self.relay_telemetry.dropped_messages.saturating_add(1);
                    }
                } else if let Some(msg) = NetMessage::from_bytes(&data) {
                    self.pending_messages.push((peer_id_str, None, msg));
                } else {
                    self.relay_telemetry.dropped_messages =
                        self.relay_telemetry.dropped_messages.saturating_add(1);
                }
            }

            let local_id = socket.id();
            if let Some(local_id) = local_id {
                let local_id_str = format!("{:?}", local_id);
                let hash = Self::hash_peer_id(&local_id_str);
                self.local_peer_hash = Some(hash);
                self.peer_hash_lookup.insert(hash, local_id_str);
            }
            // Keep transport connectivity sourced from socket state directly.
            // Message-age based filtering can create "connected but blind" states where
            // topology drops valid links before gameplay traffic has a chance to flow.
            let connected_peers: Vec<_> = socket.connected_peers().collect();
            let mut known_peers: Vec<_> = if self.discovery_attached {
                socket.known_peers().collect()
            } else {
                connected_peers.clone()
            };
            known_peers.extend(connected_peers.iter().copied());
            Self::sort_peer_ids(&mut known_peers);
            known_peers.dedup();
            Self::cap_discovery_peers(local_id, &connected_peers, &mut known_peers);
            let departures = socket.take_signaling_departures();
            (local_id, connected_peers, known_peers, departures)
        };

        // The signaling server broadcasts departures room-wide: the strongest
        // fast evidence that the topology root is gone.
        if let Some(root) = self.super_root_id {
            if departures.contains(&root) && !self.is_host && self.root_departure_frame.is_none() {
                Self::log_warn(&format!(
                    "Root {:?} left signaling; arming fast failover",
                    root
                ));
                self.root_departure_frame = Some(current_frame);
            }
        }
        self.intentional_drops
            .retain(|_, frame| current_frame.saturating_sub(*frame) < 600);

        if !connected_peers.is_empty() && self.first_peer_frame.is_none() {
            self.first_peer_frame = Some(current_frame);
        }
        if !connected_peers.is_empty() {
            self.rejoin_attempts = 0;
            self.next_rejoin_frame = 0;
        }

        let local_peer_str = local_id.map(|id| format!("{:?}", id));
        let known_peer_strs: HashSet<String> =
            known_peers.iter().map(|id| format!("{:?}", id)).collect();
        self.peer_id_lookup.retain(|peer_id, _| {
            known_peer_strs.contains(peer_id)
                || self.roster_peer_strs.contains(peer_id)
                || local_peer_str
                    .as_ref()
                    .map(|local| local == peer_id)
                    .unwrap_or(false)
        });
        self.peer_hash_lookup.retain(|_, peer_id| {
            known_peer_strs.contains(peer_id)
                || self.roster_peer_strs.contains(peer_id)
                || local_peer_str
                    .as_ref()
                    .map(|local| local == peer_id)
                    .unwrap_or(false)
                || self.remote_players.contains_key(peer_id)
                || self.pending_player_names.contains_key(peer_id)
                || self.remote_stats.contains_key(peer_id)
                || self
                    .pending_input_frames
                    .iter()
                    .any(|(id, _)| id == peer_id)
        });
        self.peer_identity_lookup.retain(|_, peer_id| {
            known_peer_strs.contains(peer_id)
                || local_peer_str
                    .as_ref()
                    .map(|local| local == peer_id)
                    .unwrap_or(false)
                || self.remote_players.contains_key(peer_id)
                || self.pending_player_names.contains_key(peer_id)
                || self.remote_stats.contains_key(peer_id)
                || self
                    .pending_input_frames
                    .iter()
                    .any(|(id, _)| id == peer_id)
        });
        let known_peer_ids: HashSet<matchbox_socket::PeerId> =
            known_peers.iter().copied().collect();
        self.last_peer_message_frames
            .retain(|peer_id, _| known_peer_ids.contains(peer_id));
        self.peer_connected_frames
            .retain(|peer_id, _| known_peer_ids.contains(peer_id));

        // Process pending messages
        let mut ping_replies: Vec<(PeerId, Ping)> = Vec::new();
        let mut pong_updates: Vec<(PeerId, Pong)> = Vec::new();
        let mut score_updates: Vec<(PeerId, SupernodeScore)> = Vec::new();
        let mut enemy_syncs: Vec<(PeerId, u64, EnemySync)> = Vec::new();
        let mut enemy_damages: Vec<(PeerId, u64, EnemyDamage)> = Vec::new();
        let mut wave_starts: Vec<(PeerId, u64, WaveStart)> = Vec::new();
        let mut enemy_kills: Vec<(PeerId, u64, EnemyKill)> = Vec::new();
        let mut player_deaths: Vec<(PeerId, u64, PlayerDeath)> = Vec::new();
        let mut paid_obstacles: Vec<(PeerId, u64, PaidObstacle)> = Vec::new();
        let mut paid_abilities: Vec<(PeerId, u64, PaidAbility)> = Vec::new();
        let mut paid_obstacle_acks: Vec<(PeerId, u64, PaidObstacleAck)> = Vec::new();
        let mut paid_ability_acks: Vec<(PeerId, u64, PaidAbilityAck)> = Vec::new();
        let mut paid_names: Vec<(PeerId, u64, PaidNameReservation)> = Vec::new();
        let mut paid_name_acks: Vec<(PeerId, u64, PaidNameAck)> = Vec::new();
        let mut cannon_shots: Vec<(PeerId, u64, CannonShot)> = Vec::new();
        let mut projectile_reflections: Vec<(PeerId, u64, ProjectileReflection)> = Vec::new();
        let mut chat_messages: Vec<(PeerId, u64, ChatMessage)> = Vec::new();
        let mut vote_mutes: Vec<(PeerId, u64, VoteMute)> = Vec::new();
        let mut player_stats_updates: Vec<(PeerId, u64, PlayerStatsSnapshot)> = Vec::new();
        let mut player_updates: Vec<(PeerId, Option<u64>, PlayerState)> = Vec::new();
        let mut input_frames: Vec<(PeerId, InputFrame)> = Vec::new();
        let mut player_batches: Vec<(PeerId, PlayerStateBatch)> = Vec::new();
        let mut input_batches: Vec<(PeerId, InputFrameBatch)> = Vec::new();
        let mut player_joins: Vec<(PeerId, u64, String)> = Vec::new();
        let mut player_lefts: Vec<(PeerId, u64)> = Vec::new();
        let mut topology_updates: Vec<(PeerId, u64, TopologyUpdate)> = Vec::new();
        let mut topology_deltas: Vec<(PeerId, TopologyDelta)> = Vec::new();
        let mut area_authority_updates: Vec<(PeerId, u64, AreaAuthorityUpdate)> = Vec::new();
        let mut join_requests: Vec<(PeerId, JoinRequest)> = Vec::new();

        for (peer_id, relay_origin_hash, msg) in self.pending_messages.drain(..) {
            let origin_hash = relay_origin_hash.unwrap_or_else(|| Self::hash_peer_id(&peer_id));
            match msg {
                NetMessage::PlayerUpdate(state) => {
                    player_updates.push((peer_id, relay_origin_hash, state));
                }
                NetMessage::PlayerJoined(name) => {
                    player_joins.push((peer_id, origin_hash, name));
                }
                NetMessage::PlayerLeft => {
                    player_lefts.push((peer_id, origin_hash));
                }
                NetMessage::EnemySync(sync) => {
                    enemy_syncs.push((peer_id, origin_hash, sync));
                }
                NetMessage::EnemyDamageEvent(damage) => {
                    enemy_damages.push((peer_id, origin_hash, damage));
                }
                NetMessage::WaveStartEvent(wave_start) => {
                    wave_starts.push((peer_id, origin_hash, wave_start));
                }
                NetMessage::EnemyKillEvent(kill) => {
                    enemy_kills.push((peer_id, origin_hash, kill));
                }
                NetMessage::EnemyKillBatchEvent(batch) => {
                    for kill in batch.kills {
                        enemy_kills.push((peer_id.clone(), origin_hash, kill));
                    }
                }
                NetMessage::PlayerDeathEvent(death) => {
                    player_deaths.push((peer_id, origin_hash, death));
                }
                NetMessage::PaidObstacleEvent(obstacle) => {
                    paid_obstacles.push((peer_id, origin_hash, obstacle));
                }
                NetMessage::PaidAbilityEvent(ability) => {
                    paid_abilities.push((peer_id, origin_hash, ability));
                }
                NetMessage::PaidObstacleSyncEvent(sync) => {
                    for obstacle in sync.obstacles {
                        self.pending_paid_obstacles
                            .push(("sync".to_string(), obstacle));
                    }
                }
                NetMessage::PaidObstacleAckEvent(ack) => {
                    paid_obstacle_acks.push((peer_id, origin_hash, ack));
                }
                NetMessage::PaidAbilityAckEvent(ack) => {
                    paid_ability_acks.push((peer_id, origin_hash, ack));
                }
                NetMessage::PaidNameReservationEvent(reservation) => {
                    paid_names.push((peer_id, origin_hash, reservation));
                }
                NetMessage::PaidNameSyncEvent(sync) => {
                    for reservation in sync.reservations {
                        self.pending_paid_names
                            .push(("sync".to_string(), reservation));
                    }
                }
                NetMessage::PaidNameAckEvent(ack) => {
                    paid_name_acks.push((peer_id, origin_hash, ack));
                }
                NetMessage::CannonShotEvent(shot) => {
                    cannon_shots.push((peer_id, origin_hash, shot));
                }
                NetMessage::ProjectileReflectionEvent(reflection) => {
                    projectile_reflections.push((peer_id, origin_hash, reflection));
                }
                NetMessage::ChatMessageEvent(chat) => {
                    chat_messages.push((peer_id, origin_hash, chat));
                }
                NetMessage::VoteMuteEvent(vote) => {
                    vote_mutes.push((peer_id, origin_hash, vote));
                }
                NetMessage::PlayerStatsEvent(stats) => {
                    player_stats_updates.push((peer_id, origin_hash, stats));
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
                    topology_updates.push((peer_id, origin_hash, update));
                }
                NetMessage::AreaAuthorityUpdateEvent(update) => {
                    area_authority_updates.push((peer_id, origin_hash, update));
                }
                NetMessage::JoinRequestEvent(request) => {
                    join_requests.push((peer_id, request));
                }
                NetMessage::TopologyDeltaEvent(delta) => {
                    topology_deltas.push((peer_id, delta));
                }
            }
        }

        for (peer_id, _origin_hash, update) in topology_updates {
            // The map is identical for every recipient, so we apply first
            // (which refreshes our own children) and then forward verbatim.
            let sender_id = self.resolve_peer_id(&peer_id);
            if self.apply_topology_map(&peer_id, &update, current_frame) {
                self.forward_topology_map(&update, sender_id);
            }
        }
        for (peer_id, delta) in topology_deltas {
            let sender_id = self.resolve_peer_id(&peer_id);
            if self.apply_topology_delta(&peer_id, &delta, current_frame) {
                self.forward_topology_delta(&delta, sender_id);
            }
        }
        for (peer_id, request) in join_requests {
            self.handle_join_request(&peer_id, request, current_frame);
        }
        for (peer_id, origin_hash, update) in area_authority_updates {
            let from_parent = self.is_parent_sender(&peer_id);
            let from_root = self.is_authoritative_sender(&peer_id);
            if (from_parent || from_root) && !self.relay_children.is_empty() {
                self.send_downstream_or_broadcast_routed(
                    NetMessage::AreaAuthorityUpdateEvent(update.clone()).to_bytes(),
                    self.resolve_peer_id(&peer_id),
                    Some(origin_hash),
                );
            }
            self.apply_area_authority_update(&peer_id, update);
        }
        for (peer_id, origin_hash, name) in player_joins {
            self.relay_control_message(
                &peer_id,
                NetMessage::PlayerJoined(name.clone()).to_bytes(),
                Some(origin_hash),
            );

            let mapped_peer_id = self.resolve_or_register_peer_hash(origin_hash);
            // Only merge aliases when the direct sender IS the origin; for a
            // relayed join the sender is just the relayer, and migrating its
            // state onto the origin's key would corrupt the relayer's entry.
            if Self::hash_peer_id(&peer_id) == origin_hash {
                self.migrate_peer_alias(&peer_id, &mapped_peer_id);
            }
            if self.is_local_peer_str(&mapped_peer_id) {
                continue;
            }
            web_sys::console::log_1(
                &format!("Player joined: {} ({})", name, mapped_peer_id).into(),
            );
            if let Some(remote) = self.remote_players.get_mut(&mapped_peer_id) {
                remote.name = name;
            } else {
                self.pending_player_names.insert(mapped_peer_id, name);
            }
        }

        for (peer_id, origin_hash) in player_lefts {
            let mapped_peer_id = self
                .identity_peer_id_for_hash(origin_hash)
                .unwrap_or(peer_id);
            self.remote_players.remove(&mapped_peer_id);
            self.remote_stats.remove(&mapped_peer_id);
            self.pending_player_names.remove(&mapped_peer_id);
            self.peer_identity_lookup.remove(&origin_hash);
        }

        for (peer_id, relay_origin, state) in player_updates {
            let peer_hash = Self::hash_peer_id(&peer_id);
            let mapped_peer_id = self.resolve_or_register_peer_hash(peer_hash);
            self.migrate_peer_alias(&peer_id, &mapped_peer_id);
            if !self.is_parent_sender(&peer_id) {
                self.queue_relay_player_state(peer_hash, state);
            }
            if self.is_local_peer_str(&mapped_peer_id) {
                continue;
            }
            // A direct (un-relayed) PlayerUpdate is the sender's own state claim;
            // accept it regardless of tree role so directly-linked peers are never
            // "connected but blind" during topology convergence.
            let direct_self_claim = relay_origin.is_none();
            if direct_self_claim
                || self.is_host
                || self.is_authoritative_sender(&peer_id)
                || self.is_parent_sender(&peer_id)
                || self.bootstrap_accept_sender(&peer_id)
            {
                if let Some(remote) = self.remote_players.get_mut(&mapped_peer_id) {
                    remote.update_state(&state, current_frame);
                } else {
                    let name = self
                        .pending_player_names
                        .remove(&mapped_peer_id)
                        .unwrap_or_else(|| "Player".to_string());
                    self.remote_players.insert(
                        mapped_peer_id.clone(),
                        RemotePlayer::new(name, &state, current_frame),
                    );
                }
            }
        }

        for (peer_id, frame) in input_frames {
            let peer_hash = Self::hash_peer_id(&peer_id);
            let mapped_peer_id = self.resolve_or_register_peer_hash(peer_hash);
            self.migrate_peer_alias(&peer_id, &mapped_peer_id);
            if !self.is_parent_sender(&peer_id) {
                self.queue_relay_input_frame(peer_hash, frame);
            }
            self.pending_input_frames.push((mapped_peer_id, frame));
        }

        for (peer_id, batch) in player_batches {
            let from_parent = self.is_parent_sender(&peer_id);
            let from_child = self.is_child_sender(&peer_id);
            if !(from_parent
                || from_child
                || self.is_host
                || self.is_authoritative_sender(&peer_id))
            {
                continue;
            }
            for entry in &batch.entries {
                // Root liveness: a member whose state is still flowing up the
                // tree is alive, even at depth >= 2 where it never talks to
                // the root directly.
                self.note_member_seen(entry.peer_hash, current_frame);
                let peer_id = self.resolve_or_register_peer_hash(entry.peer_hash);
                if self.is_local_peer_str(&peer_id) {
                    continue;
                }
                if let Some(remote) = self.remote_players.get_mut(&peer_id) {
                    remote.update_state(&entry.state, current_frame);
                } else {
                    let name = self
                        .pending_player_names
                        .remove(&peer_id)
                        .unwrap_or_else(|| "Player".to_string());
                    self.remote_players.insert(
                        peer_id.clone(),
                        RemotePlayer::new(name, &entry.state, current_frame),
                    );
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
            if !(from_parent
                || from_child
                || self.is_host
                || self.is_authoritative_sender(&peer_id))
            {
                continue;
            }
            for entry in &batch.entries {
                self.note_member_seen(entry.peer_hash, current_frame);
                let peer_id = self.resolve_or_register_peer_hash(entry.peer_hash);
                self.pending_input_frames.push((peer_id, entry.frame));
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
                self.supernode_scores.insert(
                    *peer_id,
                    (score.score_ms, score.sample_count, current_frame),
                );
            }
        }
        for (peer_id, origin_hash, damage) in enemy_damages {
            if self.is_host {
                self.pending_enemy_damage.push(damage);
            } else if !self.is_parent_sender(&peer_id) {
                self.send_upstream_or_broadcast_routed(
                    NetMessage::EnemyDamageEvent(damage).to_bytes(),
                    self.resolve_peer_id(&peer_id),
                    Some(origin_hash),
                );
            }
        }
        for (peer_id, origin_hash, sync) in enemy_syncs {
            if Some(origin_hash) == self.local_origin_hash() {
                continue; // our own correction echoed back
            }
            let root_hash = self
                .super_root_id
                .map(Self::peer_hash_for_matchbox)
                .unwrap_or(0);
            let from_host = root_hash != 0 && origin_hash == root_hash;
            // Flood along tree edges (minus the inbound one) so every node
            // hears every authority's corrections exactly once.
            self.relay_control_message(
                &peer_id,
                NetMessage::EnemySync(sync.clone()).to_bytes(),
                Some(origin_hash),
            );
            // Per-area authority enforcement: an origin may only correct
            // enemies inside areas it owns; unassigned areas belong to the
            // host (which also covers bootstrap, before any area map exists).
            let mut corrections = sync.clone();
            corrections.enemies.retain(|enemy| {
                self.area_owned_by(Self::area_id_from_pos(enemy.pos()), origin_hash)
            });
            if from_host || !corrections.enemies.is_empty() {
                self.pending_enemy_syncs.push(PendingEnemySync {
                    origin_hash,
                    from_host,
                    introductions_only: false,
                    sync: corrections,
                });
            }
            if from_host {
                let mut introductions = sync;
                introductions.enemies.retain(|enemy| {
                    !self.area_owned_by(Self::area_id_from_pos(enemy.pos()), origin_hash)
                });
                if !introductions.enemies.is_empty() {
                    self.pending_enemy_syncs.push(PendingEnemySync {
                        origin_hash,
                        from_host: true,
                        introductions_only: true,
                        sync: introductions,
                    });
                }
            }
        }
        for (peer_id, origin_hash, wave_start) in wave_starts {
            if !self.is_host
                && (self.is_authoritative_sender(&peer_id)
                    || self.is_parent_sender(&peer_id)
                    || self.bootstrap_accept_sender(&peer_id))
            {
                self.pending_wave_start = Some(wave_start);
                self.forward_wave_start_down(
                    wave_start,
                    self.resolve_peer_id(&peer_id),
                    Some(origin_hash),
                );
            } else if !self.is_host {
                self.relay_telemetry.dropped_messages =
                    self.relay_telemetry.dropped_messages.saturating_add(1);
                web_sys::console::log_1(
                    &format!("Dropped wave start from non-authoritative {}", peer_id).into(),
                );
            }
        }
        for (peer_id, origin_hash, kill) in enemy_kills {
            self.handle_enemy_kill(peer_id, origin_hash, kill, current_frame);
        }
        for (peer_id, origin_hash, death) in player_deaths {
            self.handle_player_death(peer_id, origin_hash, death, current_frame);
        }
        for (peer_id, origin_hash, obstacle) in paid_obstacles {
            let from_parent = self.is_parent_sender(&peer_id);
            let from_root = self.is_authoritative_sender(&peer_id);
            let from_child = self.is_child_sender(&peer_id);
            if self.is_host {
                self.pending_paid_obstacles
                    .push((peer_id.clone(), obstacle));
                let exclude = self.resolve_peer_id(&peer_id);
                self.relay_paid_obstacle(obstacle, exclude, Some(origin_hash));
            } else if from_root || from_parent {
                let exclude = self.resolve_peer_id(&peer_id);
                self.pending_paid_obstacles.push((peer_id, obstacle));
                if from_parent && !self.relay_children.is_empty() {
                    self.relay_paid_obstacle(obstacle, exclude, Some(origin_hash));
                }
            } else if from_child {
                self.send_upstream_or_broadcast_routed(
                    NetMessage::PaidObstacleEvent(obstacle).to_bytes(),
                    self.resolve_peer_id(&peer_id),
                    Some(origin_hash),
                );
            }
        }
        for (peer_id, origin_hash, ability) in paid_abilities {
            let from_parent = self.is_parent_sender(&peer_id);
            let from_root = self.is_authoritative_sender(&peer_id);
            let from_child = self.is_child_sender(&peer_id);
            if self.is_host {
                self.pending_paid_abilities.push((peer_id.clone(), ability));
                let exclude = self.resolve_peer_id(&peer_id);
                self.relay_paid_ability(ability, exclude, Some(origin_hash));
            } else if from_root || from_parent {
                let exclude = self.resolve_peer_id(&peer_id);
                self.pending_paid_abilities.push((peer_id, ability));
                if from_parent && !self.relay_children.is_empty() {
                    self.relay_paid_ability(ability, exclude, Some(origin_hash));
                }
            } else if from_child {
                self.send_upstream_or_broadcast_routed(
                    NetMessage::PaidAbilityEvent(ability).to_bytes(),
                    self.resolve_peer_id(&peer_id),
                    Some(origin_hash),
                );
            }
        }
        for (peer_id, origin_hash, reservation) in paid_names {
            let from_parent = self.is_parent_sender(&peer_id);
            let from_root = self.is_authoritative_sender(&peer_id);
            let from_child = self.is_child_sender(&peer_id);
            if self.is_host {
                self.pending_paid_names.push((peer_id.clone(), reservation));
                let exclude = self.resolve_peer_id(&peer_id);
                self.relay_paid_name(reservation, exclude, Some(origin_hash));
            } else if from_root || from_parent {
                let exclude = self.resolve_peer_id(&peer_id);
                self.pending_paid_names.push((peer_id, reservation));
                if from_parent && !self.relay_children.is_empty() {
                    self.relay_paid_name(reservation, exclude, Some(origin_hash));
                }
            } else if from_child {
                self.send_upstream_or_broadcast_routed(
                    NetMessage::PaidNameReservationEvent(reservation).to_bytes(),
                    self.resolve_peer_id(&peer_id),
                    Some(origin_hash),
                );
            }
        }
        for (peer_id, origin_hash, shot) in cannon_shots {
            let from_parent = self.is_parent_sender(&peer_id);
            let from_root = self.is_authoritative_sender(&peer_id);
            let from_child = self.is_child_sender(&peer_id);
            if from_root || from_parent {
                self.pending_cannon_shots.push(shot);
                if from_parent && !self.relay_children.is_empty() {
                    self.send_downstream_or_broadcast_routed(
                        NetMessage::CannonShotEvent(shot).to_bytes(),
                        self.resolve_peer_id(&peer_id),
                        Some(origin_hash),
                    );
                }
            } else if from_child {
                self.send_upstream_or_broadcast_routed(
                    NetMessage::CannonShotEvent(shot).to_bytes(),
                    self.resolve_peer_id(&peer_id),
                    Some(origin_hash),
                );
            }
        }
        for (peer_id, origin_hash, reflection) in projectile_reflections {
            let from_parent = self.is_parent_sender(&peer_id);
            let from_root = self.is_authoritative_sender(&peer_id);
            let from_child = self.is_child_sender(&peer_id);
            if self.is_host {
                self.pending_projectile_reflections.push(reflection);
                self.send_downstream_or_broadcast_routed(
                    NetMessage::ProjectileReflectionEvent(reflection).to_bytes(),
                    self.resolve_peer_id(&peer_id),
                    Some(origin_hash),
                );
            } else if from_root || from_parent {
                self.pending_projectile_reflections.push(reflection);
                if from_parent && !self.relay_children.is_empty() {
                    self.send_downstream_or_broadcast_routed(
                        NetMessage::ProjectileReflectionEvent(reflection).to_bytes(),
                        self.resolve_peer_id(&peer_id),
                        Some(origin_hash),
                    );
                }
            } else if from_child {
                self.send_upstream_or_broadcast_routed(
                    NetMessage::ProjectileReflectionEvent(reflection).to_bytes(),
                    self.resolve_peer_id(&peer_id),
                    Some(origin_hash),
                );
            }
        }
        for (peer_id, origin_hash, ack) in paid_obstacle_acks {
            let from_parent = self.is_parent_sender(&peer_id);
            let from_root = self.is_authoritative_sender(&peer_id);
            let from_child = self.is_child_sender(&peer_id);
            self.pending_paid_obstacle_acks.push((peer_id.clone(), ack));
            if self.is_host {
                self.send_low_priority_downstream_or_broadcast_routed(
                    LowPriorityTopic::Ack,
                    NetMessage::PaidObstacleAckEvent(ack).to_bytes(),
                    self.resolve_peer_id(&peer_id),
                    Some(origin_hash),
                );
            } else if from_parent {
                if !self.relay_children.is_empty() {
                    self.send_low_priority_downstream_or_broadcast_routed(
                        LowPriorityTopic::Ack,
                        NetMessage::PaidObstacleAckEvent(ack).to_bytes(),
                        self.resolve_peer_id(&peer_id),
                        Some(origin_hash),
                    );
                }
            } else if from_child {
                self.send_low_priority_upstream_or_broadcast_routed(
                    LowPriorityTopic::Ack,
                    NetMessage::PaidObstacleAckEvent(ack).to_bytes(),
                    self.resolve_peer_id(&peer_id),
                    Some(origin_hash),
                );
            } else if !from_root {
                // Ignore unknown senders after recording; forwarded copies still need tree path.
                self.send_low_priority_upstream_or_broadcast_routed(
                    LowPriorityTopic::Ack,
                    NetMessage::PaidObstacleAckEvent(ack).to_bytes(),
                    self.resolve_peer_id(&peer_id),
                    Some(origin_hash),
                );
            }
        }
        for (peer_id, origin_hash, ack) in paid_ability_acks {
            let from_parent = self.is_parent_sender(&peer_id);
            let from_root = self.is_authoritative_sender(&peer_id);
            let from_child = self.is_child_sender(&peer_id);
            self.pending_paid_ability_acks.push((peer_id.clone(), ack));
            if self.is_host {
                self.send_low_priority_downstream_or_broadcast_routed(
                    LowPriorityTopic::Ack,
                    NetMessage::PaidAbilityAckEvent(ack).to_bytes(),
                    self.resolve_peer_id(&peer_id),
                    Some(origin_hash),
                );
            } else if from_parent {
                if !self.relay_children.is_empty() {
                    self.send_low_priority_downstream_or_broadcast_routed(
                        LowPriorityTopic::Ack,
                        NetMessage::PaidAbilityAckEvent(ack).to_bytes(),
                        self.resolve_peer_id(&peer_id),
                        Some(origin_hash),
                    );
                }
            } else if from_child {
                self.send_low_priority_upstream_or_broadcast_routed(
                    LowPriorityTopic::Ack,
                    NetMessage::PaidAbilityAckEvent(ack).to_bytes(),
                    self.resolve_peer_id(&peer_id),
                    Some(origin_hash),
                );
            } else if !from_root {
                self.send_low_priority_upstream_or_broadcast_routed(
                    LowPriorityTopic::Ack,
                    NetMessage::PaidAbilityAckEvent(ack).to_bytes(),
                    self.resolve_peer_id(&peer_id),
                    Some(origin_hash),
                );
            }
        }
        for (peer_id, origin_hash, ack) in paid_name_acks {
            let from_parent = self.is_parent_sender(&peer_id);
            let from_root = self.is_authoritative_sender(&peer_id);
            let from_child = self.is_child_sender(&peer_id);
            self.pending_paid_name_acks.push((peer_id.clone(), ack));
            if self.is_host {
                self.send_low_priority_downstream_or_broadcast_routed(
                    LowPriorityTopic::Ack,
                    NetMessage::PaidNameAckEvent(ack).to_bytes(),
                    self.resolve_peer_id(&peer_id),
                    Some(origin_hash),
                );
            } else if from_parent {
                if !self.relay_children.is_empty() {
                    self.send_low_priority_downstream_or_broadcast_routed(
                        LowPriorityTopic::Ack,
                        NetMessage::PaidNameAckEvent(ack).to_bytes(),
                        self.resolve_peer_id(&peer_id),
                        Some(origin_hash),
                    );
                }
            } else if from_child {
                self.send_low_priority_upstream_or_broadcast_routed(
                    LowPriorityTopic::Ack,
                    NetMessage::PaidNameAckEvent(ack).to_bytes(),
                    self.resolve_peer_id(&peer_id),
                    Some(origin_hash),
                );
            } else if !from_root {
                self.send_low_priority_upstream_or_broadcast_routed(
                    LowPriorityTopic::Ack,
                    NetMessage::PaidNameAckEvent(ack).to_bytes(),
                    self.resolve_peer_id(&peer_id),
                    Some(origin_hash),
                );
            }
        }
        for (_, _, chat) in &chat_messages {
            if !self.is_muted(chat.sender_hash) {
                self.pending_chat_messages.push(chat.clone());
            }
        }
        for (_, _, vote) in &vote_mutes {
            if self.register_vote_mute(*vote) {
                self.pending_vote_mutes.push(*vote);
            }
        }
        for (_, _, snapshot) in &player_stats_updates {
            self.apply_remote_stats_snapshot(*snapshot);
        }
        for (peer_id, origin_hash, chat) in chat_messages {
            self.relay_low_priority_control_message(
                &peer_id,
                LowPriorityTopic::Chat,
                NetMessage::ChatMessageEvent(chat).to_bytes(),
                Some(origin_hash),
            );
        }
        for (peer_id, origin_hash, vote) in vote_mutes {
            self.relay_low_priority_control_message(
                &peer_id,
                LowPriorityTopic::Vote,
                NetMessage::VoteMuteEvent(vote).to_bytes(),
                Some(origin_hash),
            );
        }
        for (peer_id, origin_hash, snapshot) in player_stats_updates {
            self.relay_low_priority_control_message(
                &peer_id,
                LowPriorityTopic::Stats,
                NetMessage::PlayerStatsEvent(snapshot).to_bytes(),
                Some(origin_hash),
            );
        }

        // Never keep the local player in the remote roster, even if a relayed/batched
        // echo slips through during topology transitions.
        if let Some(local) = self.local_peer_id.or(local_id) {
            let local_key = format!("{:?}", local);
            self.remote_players.remove(&local_key);
            self.remote_stats.remove(&local_key);
            self.pending_player_names.remove(&local_key);
        }

        // Update interpolation for all remote players
        for remote in self.remote_players.values_mut() {
            remote.update();
        }

        // Remove stale players
        self.remote_players
            .retain(|_, p| !p.is_stale(current_frame));
        self.remote_stats
            .retain(|peer_id, _| self.remote_players.contains_key(peer_id));
        self.pending_player_names
            .retain(|peer_id, _| !self.remote_players.contains_key(peer_id));

        let prev_supernode = self.supernode_id;
        if let Some(id) = local_id {
            self.local_peer_id = Some(id);
        }
        self.supernode_scores
            .retain(|peer_id, _| connected_peers.contains(peer_id));
        self.latency_ms
            .retain(|peer_id, _| connected_peers.contains(peer_id));
        self.latency_samples
            .retain(|peer_id, _| connected_peers.contains(peer_id));

        // Topology: the root is the single authority; everyone else adopts its
        // map and never self-elects (except the explicit failover paths below).
        if self.is_host {
            self.update_topology_as_root(current_frame, &connected_peers);
        } else {
            self.maybe_send_join_request(current_frame, &connected_peers);
            self.maybe_root_failover(current_frame, &connected_peers);
        }
        self.maybe_failover_parent(current_frame);
        self.maybe_backoff_silent_peers(current_frame, &connected_peers);
        self.update_desired_peers(&known_peers, &connected_peers);
        if prev_supernode != self.supernode_id {
            web_sys::console::log_1(
                &format!(
                    "Supernode updated: {:?} -> {:?}",
                    prev_supernode, self.supernode_id
                )
                .into(),
            );
        }

        if !connected_peers.is_empty() {
            self.state = NetworkState::Connected;
        } else if self.remote_players.is_empty() {
            self.state = NetworkState::WaitingForPeers;
        }
        self.log_sync_trace(current_frame, &connected_peers, &known_peers);
        self.prune_event_confirmations(current_frame);
        self.maybe_reannounce_name(current_frame, &connected_peers);
        self.tick_latency(current_frame, &connected_peers);
        self.log_telemetry_periodic(current_frame);

        true
    }

    /// Periodically re-flood our display name through the tree. Heals races
    /// where the connect-time PlayerJoined was missed and covers members we
    /// will never be directly linked to (siblings, other subtrees).
    fn maybe_reannounce_name(
        &mut self,
        current_frame: u32,
        connected_peers: &[matchbox_socket::PeerId],
    ) {
        const NAME_ANNOUNCE_PERIOD_FRAMES: u32 = 600;
        if connected_peers.is_empty() {
            return;
        }
        if self.last_name_announce_frame != 0
            && current_frame.saturating_sub(self.last_name_announce_frame)
                < NAME_ANNOUNCE_PERIOD_FRAMES
        {
            return;
        }
        self.last_name_announce_frame = current_frame;
        let msg = NetMessage::PlayerJoined(self.local_player_name.clone()).to_bytes();
        let origin = self.local_origin_hash();
        if !self.is_host {
            self.send_upstream_or_broadcast_routed(msg.clone(), None, origin);
        }
        if !self.relay_children.is_empty() || self.is_host {
            self.send_downstream_or_broadcast_routed(msg, None, origin);
        }
    }

    /// Every hash reachable from `start` by following child edges (including
    /// `start` itself). Used to avoid creating cycles when reparenting.
    fn roster_subtree(&self, start: u64) -> HashSet<u64> {
        let mut subtree = HashSet::new();
        subtree.insert(start);
        let mut frontier = vec![start];
        while let Some(node) = frontier.pop() {
            for entry in &self.topology_roster {
                if entry.parent_hash == node && subtree.insert(entry.peer_hash) {
                    frontier.push(entry.peer_hash);
                }
            }
        }
        subtree
    }

    fn pick_parent_excluding(&self, exclude: &HashSet<u64>) -> u64 {
        let fanout = self.relay_fanout.clamp(1, Self::MAX_FANOUT);
        for entry in &self.topology_roster {
            if exclude.contains(&entry.peer_hash) {
                continue;
            }
            if self.roster_child_count(entry.peer_hash) < fanout {
                return entry.peer_hash;
            }
        }
        self.roster_root_hash().unwrap_or(0)
    }

    /// Root: drop a member and re-home its children (only the orphaned subtree
    /// roots move; everything else keeps its routes).
    fn remove_roster_member(&mut self, peer_hash: u64, current_frame: u32) {
        let Some(pos) = self
            .topology_roster
            .iter()
            .position(|entry| entry.peer_hash == peer_hash)
        else {
            return;
        };
        self.topology_roster.remove(pos);
        self.member_last_seen.remove(&peer_hash);
        self.member_parent_since.remove(&peer_hash);
        self.member_link_nags.remove(&peer_hash);
        let orphans: Vec<u64> = self
            .topology_roster
            .iter()
            .filter(|entry| entry.parent_hash == peer_hash)
            .map(|entry| entry.peer_hash)
            .collect();
        for orphan in orphans {
            let subtree = self.roster_subtree(orphan);
            let new_parent = self.pick_parent_excluding(&subtree);
            if let Some(entry) = self
                .topology_roster
                .iter_mut()
                .find(|entry| entry.peer_hash == orphan)
            {
                entry.parent_hash = new_parent;
            }
            self.member_parent_since.insert(orphan, current_frame);
        }
        self.roster_dirty = true;
    }

    /// Root: keep the roster current (admit connected strangers, prune dead
    /// members), derive our own links, and broadcast the map on change or on
    /// the heartbeat cadence.
    fn update_topology_as_root(
        &mut self,
        current_frame: u32,
        connected_peers: &[matchbox_socket::PeerId],
    ) {
        let Some(local_id) = self.local_peer_id else {
            return;
        };
        let local_uuid = Self::uuid_for_peer(local_id);
        let (local_hash, _) = self.register_peer_uuid(local_uuid);
        self.local_peer_hash = Some(local_hash);

        // Claim the root slot; anything else claiming it becomes our child.
        let needs_root_fix = self
            .roster_entry(local_hash)
            .map(|entry| entry.parent_hash != 0)
            .unwrap_or(true)
            || self.roster_root_hash() != Some(local_hash);
        if needs_root_fix {
            self.topology_roster
                .retain(|entry| entry.peer_hash != local_hash);
            for entry in self.topology_roster.iter_mut() {
                if entry.parent_hash == 0 {
                    entry.parent_hash = local_hash;
                }
            }
            self.topology_roster.insert(
                0,
                TopologyEntry {
                    peer_hash: local_hash,
                    uuid: local_uuid,
                    parent_hash: 0,
                },
            );
            self.roster_dirty = true;
        }

        // Match fanout to room size before admitting (new members use it).
        let fanout = self
            .fanout_override
            .unwrap_or_else(|| Self::choose_dynamic_fanout(self.topology_roster.len().max(1)))
            .clamp(1, Self::MAX_FANOUT);
        if fanout != self.relay_fanout {
            self.relay_fanout = fanout;
            self.roster_dirty = true;
        }

        // A live connection is proof of life and an implicit join request.
        for peer in connected_peers {
            self.add_roster_member(Self::uuid_for_peer(*peer), current_frame);
        }

        // Prune members with no signs of life (their traffic refreshes the TTL
        // through relayed batch entries even at depth >= 2).
        let connected_hashes: HashSet<u64> = connected_peers
            .iter()
            .map(|peer| Self::peer_hash_for_matchbox(*peer))
            .collect();
        let dead: Vec<u64> = self
            .topology_roster
            .iter()
            .filter(|entry| {
                entry.peer_hash != local_hash && !connected_hashes.contains(&entry.peer_hash)
            })
            .filter(|entry| {
                let last_seen = self
                    .member_last_seen
                    .get(&entry.peer_hash)
                    .copied()
                    .unwrap_or(0);
                current_frame.saturating_sub(last_seen) > Self::MEMBER_TTL_FRAMES
            })
            .map(|entry| entry.peer_hash)
            .collect();
        for hash in dead {
            Self::log_info(&format!(
                "Root pruned silent member {:#x} from relay tree",
                hash
            ));
            self.remove_roster_member(hash, current_frame);
        }
        let roster_hashes: HashSet<u64> = self
            .topology_roster
            .iter()
            .map(|entry| entry.peer_hash)
            .collect();
        self.member_last_seen
            .retain(|hash, _| roster_hashes.contains(hash));

        if self.roster_dirty {
            self.relay_epoch = self.relay_epoch.wrapping_add(1);
        }

        self.derive_links_from_roster(current_frame);
        self.recompute_area_authorities(current_frame);

        // Heartbeats get cheaper-but-rarer in big rooms; in delta mode the
        // heartbeat is an empty delta (~30 bytes), not a full map. Membership
        // changes broadcast immediately in small rooms / when a joiner is
        // waiting, and are coalesced in big rooms.
        let heartbeat_period = if self.topology_roster.len() <= 128 {
            Self::MAP_BROADCAST_PERIOD_FRAMES
        } else {
            Self::MAP_BROADCAST_PERIOD_FRAMES * 5
        };
        let since_broadcast = current_frame.saturating_sub(self.last_topology_broadcast_frame);
        let heartbeat_due = since_broadcast >= heartbeat_period;
        let immediate_dirty = self.topology_roster.len() < Self::DELTA_ROSTER_MIN
            || !self.pending_map_peers.is_empty()
            || since_broadcast >= Self::MIN_DIRTY_BROADCAST_FRAMES;
        if (self.roster_dirty && immediate_dirty)
            || heartbeat_due
            || !self.pending_map_peers.is_empty()
        {
            self.last_topology_broadcast_frame = current_frame;
            self.roster_dirty = false;
            self.broadcast_topology();
        }
    }

    /// Everyone: derive parent/backup/children/supernode-set from the roster.
    /// Pure function of the map, so all nodes agree on every link.
    fn derive_links_from_roster(&mut self, current_frame: u32) {
        self.rebuild_roster_peer_strs();
        let Some(local_hash) = self.local_peer_hash else {
            return;
        };
        let entries = self.topology_roster.clone();
        for entry in &entries {
            self.register_peer_uuid(entry.uuid);
        }

        let root_hash = self.roster_root_hash();
        let root_id = root_hash.and_then(|hash| self.hash_to_matchbox(hash));
        self.super_root_id = root_id;
        self.supernode_id = root_id;

        // Interior nodes (anyone with children) are the supernodes.
        let parent_hashes: HashSet<u64> = entries
            .iter()
            .filter(|entry| entry.parent_hash != 0)
            .map(|entry| entry.parent_hash)
            .collect();
        let mut supernodes: Vec<matchbox_socket::PeerId> = parent_hashes
            .iter()
            .filter_map(|hash| self.hash_to_matchbox(*hash))
            .collect();
        if let Some(root) = root_id {
            if !supernodes.contains(&root) {
                supernodes.push(root);
            }
        }
        Self::sort_peer_ids(&mut supernodes);
        self.supernode_set = supernodes;

        let my_entry = entries.iter().find(|entry| entry.peer_hash == local_hash);
        let prev_parent = self.relay_parent;
        let prev_backup = self.relay_backup_parent;
        let prev_active = self.relay_active_parent;

        let parent_hash = my_entry.map(|entry| entry.parent_hash).unwrap_or(0);
        let new_parent = if parent_hash == 0 {
            None
        } else {
            self.hash_to_matchbox(parent_hash)
        };
        let grandparent_hash = entries
            .iter()
            .find(|entry| entry.peer_hash == parent_hash)
            .map(|entry| entry.parent_hash)
            .unwrap_or(0);
        let mut new_backup = if grandparent_hash != 0 {
            self.hash_to_matchbox(grandparent_hash)
        } else {
            root_id
        };
        if new_backup == new_parent || new_backup == self.local_peer_id {
            new_backup = None;
        }
        // Keep the previous parent as backup right after a reassignment: it is
        // the link most likely to still be alive during the handoff.
        if prev_parent.is_some() && prev_parent != new_parent && prev_parent != self.local_peer_id {
            new_backup = prev_parent;
        }

        self.relay_parent = new_parent;
        self.relay_backup_parent = new_backup;
        self.relay_children = entries
            .iter()
            .filter(|entry| entry.parent_hash == local_hash)
            .filter_map(|entry| self.hash_to_matchbox(entry.peer_hash))
            .collect();
        let was_host = self.is_host;
        self.is_host = my_entry
            .map(|entry| entry.parent_hash == 0)
            .unwrap_or(self.is_host);
        if self.is_host && !was_host {
            self.root_acquired_frame = current_frame;
            // Gained the root role via a map (e.g. a throttled root handed
            // off to us). Seed liveness for inherited members so the TTL
            // prune doesn't wipe everyone we aren't directly linked to, and
            // force the next broadcast to be a full-map anchor.
            for entry in &entries {
                self.member_last_seen.insert(entry.peer_hash, current_frame);
                self.member_parent_since
                    .entry(entry.peer_hash)
                    .or_insert(current_frame);
            }
            self.member_link_nags.clear();
            self.last_broadcast_roster.clear();
            self.last_broadcast_epoch = 0;
            self.root_departure_frame = None;
        }

        if self.is_host {
            self.relay_active_parent = None;
        } else if self.relay_active_parent.is_none() {
            self.relay_active_parent = self.relay_parent;
        } else if let Some(active) = self.relay_active_parent {
            let valid =
                Some(active) == self.relay_parent || Some(active) == self.relay_backup_parent;
            if !valid {
                self.relay_active_parent = self.relay_parent;
            }
        }
        if prev_parent != self.relay_parent
            || prev_backup != self.relay_backup_parent
            || prev_active != self.relay_active_parent
        {
            self.last_parent_switch_frame = current_frame;
        }
    }

    fn topology_map_message(&self) -> Option<TopologyUpdate> {
        let root_hash = self.roster_root_hash()?;
        Some(TopologyUpdate {
            epoch: self.relay_epoch,
            root_hash,
            fanout: self.relay_fanout as u8,
            entries: self.topology_roster.clone(),
        })
    }

    /// Root: send the map to every directly-connected peer (children plus any
    /// freshly admitted stragglers); they forward it down the tree.
    fn broadcast_topology_map(&mut self) {
        let Some(update) = self.topology_map_message() else {
            return;
        };
        self.pending_map_peers.clear();
        let msg = NetMessage::TopologyUpdateEvent(update).to_bytes();
        let socket = match &mut self.socket {
            Some(s) => s,
            None => return,
        };
        let peers: Vec<_> = socket.connected_peers().collect();
        for peer_id in peers {
            socket.send(msg.clone().into_boxed_slice(), peer_id);
            self.relay_telemetry.sent_downstream =
                self.relay_telemetry.sent_downstream.saturating_add(1);
        }
    }

    /// Root: broadcast the current topology. Small rooms always send the full
    /// map. Big rooms send deltas against the last broadcast snapshot, an
    /// empty delta as the liveness heartbeat, and periodic full-map anchors;
    /// new members and waiting joiners always get the full map (they have no
    /// base to apply a delta to).
    fn broadcast_topology(&mut self) {
        let Some(full) = self.topology_map_message() else {
            return;
        };
        let roster_len = self.topology_roster.len();
        let checksum = Self::roster_checksum(&self.topology_roster);
        let anchor_due = self.map_anchor_counter % Self::MAP_FULL_ANCHOR_EVERY == 0;
        self.map_anchor_counter = self.map_anchor_counter.wrapping_add(1);

        let use_full_for_all = roster_len < Self::DELTA_ROSTER_MIN
            || self.last_broadcast_roster.is_empty()
            || anchor_due;

        let delta_msg = if use_full_for_all {
            None
        } else {
            let prev: HashMap<u64, TopologyEntry> = self
                .last_broadcast_roster
                .iter()
                .map(|entry| (entry.peer_hash, *entry))
                .collect();
            let current: HashSet<u64> = self
                .topology_roster
                .iter()
                .map(|entry| entry.peer_hash)
                .collect();
            let removed: Vec<u64> = prev
                .keys()
                .copied()
                .filter(|hash| !current.contains(hash))
                .collect();
            let upserts: Vec<TopologyEntry> = self
                .topology_roster
                .iter()
                .copied()
                .filter(|entry| prev.get(&entry.peer_hash) != Some(entry))
                .collect();
            let delta = TopologyDelta {
                epoch_from: self.last_broadcast_epoch,
                epoch_to: self.relay_epoch,
                root_hash: full.root_hash,
                fanout: self.relay_fanout as u8,
                checksum,
                removed,
                upserts,
            };
            Some((
                NetMessage::TopologyDeltaEvent(delta.clone()).to_bytes(),
                delta,
            ))
        };

        let full_msg = NetMessage::TopologyUpdateEvent(full).to_bytes();
        let prev_members: HashSet<u64> = self
            .last_broadcast_roster
            .iter()
            .map(|entry| entry.peer_hash)
            .collect();
        let mut full_targets: HashSet<matchbox_socket::PeerId> =
            self.pending_map_peers.drain(..).collect();

        let socket = match &mut self.socket {
            Some(s) => s,
            None => return,
        };
        for peer_id in socket.connected_peers().collect::<Vec<_>>() {
            let needs_full = use_full_for_all
                || full_targets.contains(&peer_id)
                || !prev_members.contains(&Self::peer_hash_for_matchbox(peer_id));
            let payload = if needs_full {
                full_msg.clone()
            } else if let Some((bytes, _)) = &delta_msg {
                bytes.clone()
            } else {
                full_msg.clone()
            };
            socket.send(payload.into_boxed_slice(), peer_id);
            self.relay_telemetry.sent_downstream =
                self.relay_telemetry.sent_downstream.saturating_add(1);
            full_targets.remove(&peer_id);
        }

        self.last_broadcast_roster = self.topology_roster.clone();
        self.last_broadcast_epoch = self.relay_epoch;
    }

    /// Receiver: apply a delta on top of our roster. Any mismatch (unknown
    /// base epoch, different root, checksum divergence) falls back to
    /// requesting a full map rather than guessing. Returns true when the
    /// delta should be forwarded to our children.
    fn apply_topology_delta(
        &mut self,
        sender: &str,
        delta: &TopologyDelta,
        current_frame: u32,
    ) -> bool {
        if self.is_host {
            if Some(delta.root_hash) != self.local_peer_hash {
                // Another root's stream; show them ours so the merge protocol
                // (full maps + outranking) can resolve it.
                self.reply_with_current_map(sender);
            }
            return false;
        }
        let cur_root = self.roster_root_hash().unwrap_or(0);
        if self.topology_roster.is_empty() || delta.root_hash != cur_root {
            if delta.epoch_to > self.relay_epoch {
                self.needs_full_map = true;
            }
            return false;
        }
        let from_authority = self.is_parent_sender(sender) || self.is_supernode_sender(sender);

        // Empty heartbeat: proves root liveness and doubles as a divergence
        // detector (checksum must match what we hold).
        if delta.epoch_to == self.relay_epoch && delta.epoch_from == delta.epoch_to {
            if Self::roster_checksum(&self.topology_roster) != delta.checksum {
                self.needs_full_map = true;
                return false;
            }
            if from_authority {
                self.last_map_rx_frame = current_frame;
                self.root_departure_frame = None;
                return true;
            }
            return false;
        }

        if delta.epoch_to <= self.relay_epoch {
            return false;
        }
        if delta.epoch_from != self.relay_epoch {
            self.needs_full_map = true;
            return false;
        }

        let mut next = self.topology_roster.clone();
        next.retain(|entry| !delta.removed.contains(&entry.peer_hash));
        for upsert in &delta.upserts {
            if let Some(entry) = next
                .iter_mut()
                .find(|entry| entry.peer_hash == upsert.peer_hash)
            {
                *entry = *upsert;
            } else {
                next.push(*upsert);
            }
        }
        if Self::roster_checksum(&next) != delta.checksum {
            self.needs_full_map = true;
            return false;
        }

        self.topology_roster = next;
        self.relay_epoch = delta.epoch_to;
        self.relay_fanout = (delta.fanout as usize).clamp(Self::MIN_FANOUT, Self::MAX_FANOUT);
        // A delta always carries a new epoch: genuinely fresh root output.
        self.last_map_rx_frame = current_frame;
        self.root_departure_frame = None;
        self.needs_full_map = false;
        self.derive_links_from_roster(current_frame);
        true
    }

    /// Non-root: forward a delta to our children; waiting joiners get a full
    /// map instead (they have no base).
    fn forward_topology_delta(
        &mut self,
        delta: &TopologyDelta,
        exclude: Option<matchbox_socket::PeerId>,
    ) {
        let pending: Vec<matchbox_socket::PeerId> = self.pending_map_peers.drain(..).collect();
        if !pending.is_empty() {
            if let Some(full) = self.topology_map_message() {
                let full_msg = NetMessage::TopologyUpdateEvent(full).to_bytes();
                if let Some(socket) = &mut self.socket {
                    for peer_id in &pending {
                        socket.send(full_msg.clone().into_boxed_slice(), *peer_id);
                    }
                }
            }
        }
        let mut targets = self.relay_children.clone();
        targets.retain(|target| {
            Some(*target) != exclude
                && Some(*target) != self.local_peer_id
                && !pending.contains(target)
        });
        if targets.is_empty() {
            return;
        }
        let msg = NetMessage::TopologyDeltaEvent(delta.clone()).to_bytes();
        let socket = match &mut self.socket {
            Some(s) => s,
            None => return,
        };
        for target in targets {
            socket.send(msg.clone().into_boxed_slice(), target);
            self.relay_telemetry.sent_downstream =
                self.relay_telemetry.sent_downstream.saturating_add(1);
        }
    }

    /// Non-root: forward an accepted map verbatim to our children and anyone
    /// who recently asked us for a slot.
    fn forward_topology_map(
        &mut self,
        update: &TopologyUpdate,
        exclude: Option<matchbox_socket::PeerId>,
    ) {
        let mut targets = self.relay_children.clone();
        for peer_id in self.pending_map_peers.drain(..) {
            if !targets.contains(&peer_id) {
                targets.push(peer_id);
            }
        }
        targets.retain(|target| Some(*target) != exclude && Some(*target) != self.local_peer_id);
        if targets.is_empty() {
            return;
        }
        let msg = NetMessage::TopologyUpdateEvent(update.clone()).to_bytes();
        let socket = match &mut self.socket {
            Some(s) => s,
            None => return,
        };
        for target in targets {
            socket.send(msg.clone().into_boxed_slice(), target);
            self.relay_telemetry.sent_downstream =
                self.relay_telemetry.sent_downstream.saturating_add(1);
        }
    }

    /// Total order over competing maps so concurrent roots converge.
    fn map_outranks(new_epoch: u32, new_root: u64, cur_epoch: u32, cur_root: u64) -> bool {
        new_epoch > cur_epoch || (new_epoch == cur_epoch && new_root < cur_root)
    }

    /// Returns true when the map was adopted (and should be forwarded).
    fn apply_topology_map(
        &mut self,
        sender: &str,
        update: &TopologyUpdate,
        current_frame: u32,
    ) -> bool {
        if update.entries.is_empty() {
            return false;
        }
        let local_hash = self.local_peer_hash;
        let prev_epoch = self.relay_epoch;
        // Same-epoch copies count as a root liveness heartbeat only when they
        // arrive over the authoritative path (parent or the root itself).
        // Lateral echoes (e.g. join-request replies between two orphans of a
        // dead root) must not refresh liveness, or stranded peers would keep
        // laundering a dead root's freshness for each other and block every
        // failover path.
        let mut from_authority = self.is_parent_sender(sender) || self.is_supernode_sender(sender);

        if self.is_host {
            if Some(update.root_hash) == local_hash {
                // Our own map echoed back.
                return false;
            }
            let my_root = local_hash.unwrap_or(0);
            if !Self::map_outranks(update.epoch, update.root_hash, self.relay_epoch, my_root) {
                // We outrank the other root: tell the sender so the losing
                // root (or its subtree) hears about us and folds in.
                self.reply_with_current_map(sender);
                return false;
            }
            Self::log_warn(&format!(
                "Abdicating root to {:#x} (epoch {} vs local {})",
                update.root_hash, update.epoch, self.relay_epoch
            ));
            self.is_host = false;
            self.member_last_seen.clear();
            self.member_parent_since.clear();
            self.member_link_nags.clear();
            self.last_broadcast_roster.clear();
            self.last_broadcast_epoch = 0;
            self.roster_dirty = false;
            from_authority = true;
        } else if !self.topology_roster.is_empty() {
            let cur_root = self.roster_root_hash().unwrap_or(0);
            let same_root = update.root_hash == cur_root;
            let acceptable = if same_root {
                update.epoch >= self.relay_epoch
            } else {
                Self::map_outranks(update.epoch, update.root_hash, self.relay_epoch, cur_root)
            };
            if !acceptable {
                if !same_root {
                    self.reply_with_current_map(sender);
                }
                return false;
            }
        }

        self.topology_roster = update.entries.clone();
        self.relay_epoch = update.epoch;
        self.relay_fanout = (update.fanout as usize).clamp(Self::MIN_FANOUT, Self::MAX_FANOUT);
        let fresh_information = update.epoch > prev_epoch || from_authority;
        if fresh_information {
            self.last_map_rx_frame = current_frame;
            // A genuinely live map supersedes any death evidence about its root.
            self.root_departure_frame = None;
        }
        self.derive_links_from_roster(current_frame);
        fresh_information
    }

    fn reply_with_current_map(&mut self, sender: &str) {
        let Some(sender_id) = self.resolve_peer_id(sender) else {
            return;
        };
        let Some(update) = self.topology_map_message() else {
            return;
        };
        let msg = NetMessage::TopologyUpdateEvent(update).to_bytes();
        if let Some(socket) = &mut self.socket {
            socket.send(msg.into_boxed_slice(), sender_id);
        }
    }

    /// Root: track repeated join requests from members that already have a
    /// slot; enough of them within a window means their parent link is not
    /// forming, so move them under a different parent (cooldown applies).
    fn note_member_link_nag(&mut self, member_hash: u64, current_frame: u32) {
        let (count, last_frame) = self
            .member_link_nags
            .get(&member_hash)
            .copied()
            .unwrap_or((0, 0));
        let count = if current_frame.saturating_sub(last_frame) > Self::LINK_NAG_WINDOW_FRAMES {
            1
        } else {
            count.saturating_add(1)
        };
        self.member_link_nags
            .insert(member_hash, (count, current_frame));
        if count < Self::LINK_NAG_THRESHOLD {
            return;
        }
        let parent_age = current_frame.saturating_sub(
            self.member_parent_since
                .get(&member_hash)
                .copied()
                .unwrap_or(0),
        );
        if parent_age < Self::REASSIGN_MIN_PARENT_AGE_FRAMES {
            return;
        }
        let Some(current_parent) = self
            .roster_entry(member_hash)
            .map(|entry| entry.parent_hash)
        else {
            return;
        };
        if current_parent == 0 {
            return;
        }
        let mut exclude = self.roster_subtree(member_hash);
        exclude.insert(current_parent);
        let new_parent = self.pick_parent_excluding(&exclude);
        if new_parent == current_parent || new_parent == member_hash {
            return;
        }
        if let Some(entry) = self
            .topology_roster
            .iter_mut()
            .find(|entry| entry.peer_hash == member_hash)
        {
            entry.parent_hash = new_parent;
        }
        self.member_parent_since.insert(member_hash, current_frame);
        self.member_link_nags.remove(&member_hash);
        self.roster_dirty = true;
        Self::log_warn(&format!(
            "Reassigned member {member_hash:#x} from unreachable parent {current_parent:#x} to {new_parent:#x}"
        ));
    }

    fn handle_join_request(&mut self, sender: &str, request: JoinRequest, current_frame: u32) {
        let (origin_hash, _) = self.register_peer_uuid(request.uuid);
        if Some(origin_hash) == self.local_peer_hash {
            return;
        }
        let sender_id = self.resolve_peer_id(sender);

        if self.is_host {
            if self.roster_contains(origin_hash) {
                // An in-roster member that keeps asking for a slot has a
                // route problem: most likely the WebRTC link to its assigned
                // parent never forms (e.g. two hard NATs). Reassign it to a
                // different parent instead of letting it starve — the tree
                // itself is the relay fallback, no TURN required.
                self.note_member_link_nag(origin_hash, current_frame);
            } else {
                self.add_roster_member(request.uuid, current_frame);
            }
            self.member_last_seen.insert(origin_hash, current_frame);
            if let Some(sender_id) = sender_id {
                if !self.pending_map_peers.contains(&sender_id) {
                    self.pending_map_peers.push(sender_id);
                }
            }
            return;
        }

        // Forward up the tree (deduped per origin) so it reaches the root.
        let last_forward = self
            .recent_join_forwards
            .get(&origin_hash)
            .copied()
            .unwrap_or(0);
        let recently_forwarded = last_forward != 0
            && current_frame.saturating_sub(last_forward) < Self::JOIN_FORWARD_DEDUP_FRAMES;
        if !recently_forwarded {
            self.recent_join_forwards.insert(origin_hash, current_frame);
            self.recent_join_forwards.retain(|_, frame| {
                current_frame.saturating_sub(*frame) < Self::JOIN_FORWARD_DEDUP_FRAMES * 4
            });
            self.send_upstream_or_broadcast_routed(
                NetMessage::JoinRequestEvent(request).to_bytes(),
                sender_id,
                Some(origin_hash),
            );
        }

        // Hand the requester our current map right away so it can bootstrap,
        // and queue it for the next refresh that flows through us.
        if let Some(sender_id) = sender_id {
            if let Some(map) = self.topology_map_message() {
                let msg = NetMessage::TopologyUpdateEvent(map).to_bytes();
                if let Some(socket) = &mut self.socket {
                    socket.send(msg.into_boxed_slice(), sender_id);
                }
            }
            if !self.pending_map_peers.contains(&sender_id) {
                self.pending_map_peers.push(sender_id);
            }
        }
    }

    /// Non-root: ask for a tree slot while we don't have one (or our route died).
    fn maybe_send_join_request(
        &mut self,
        current_frame: u32,
        connected_peers: &[matchbox_socket::PeerId],
    ) {
        if connected_peers.is_empty() {
            return;
        }
        let Some(local_id) = self.local_peer_id else {
            return;
        };
        let local_hash = self
            .local_peer_hash
            .unwrap_or_else(|| Self::peer_hash_for_matchbox(local_id));
        let in_roster = self.roster_contains(local_hash);
        let route_starved = match self.relay_active_parent.or(self.relay_parent) {
            Some(parent) => {
                !self.peer_seen_recently(parent, current_frame)
                    && current_frame.saturating_sub(self.last_map_rx_frame)
                        > Self::RELAY_PARENT_STALE_FRAMES
            }
            None => true,
        };
        if !self.needs_full_map && in_roster && !route_starved {
            return;
        }
        if self.last_join_request_frame != 0
            && current_frame.saturating_sub(self.last_join_request_frame)
                < Self::JOIN_REQUEST_PERIOD_FRAMES
        {
            return;
        }
        self.last_join_request_frame = current_frame;
        self.needs_full_map = false;
        let request = JoinRequest {
            peer_hash: local_hash,
            uuid: Self::uuid_for_peer(local_id),
        };
        let msg = NetMessage::JoinRequestEvent(request).to_bytes();
        if let Some(socket) = &mut self.socket {
            for peer_id in connected_peers {
                socket.send(msg.clone().into_boxed_slice(), *peer_id);
            }
        }
    }

    /// Non-root: replace a provably dead root. All nodes share the same last
    /// map, so the successor order (uuid order, staggered takeover) is agreed
    /// without extra coordination; rare double-promotions converge via
    /// `map_outranks` + abdication.
    fn maybe_root_failover(
        &mut self,
        current_frame: u32,
        connected_peers: &[matchbox_socket::PeerId],
    ) {
        if self.is_host {
            return;
        }
        let Some(local_id) = self.local_peer_id else {
            return;
        };
        let local_hash = self
            .local_peer_hash
            .unwrap_or_else(|| Self::peer_hash_for_matchbox(local_id));

        if self.topology_roster.is_empty() {
            // Never saw a map. If nobody claims the room for a long time the
            // lowest-uuid connected node takes it (creator left early).
            if self.last_map_rx_frame != 0 {
                return;
            }
            let Some(first_peer_frame) = self.first_peer_frame else {
                return;
            };
            if current_frame.saturating_sub(first_peer_frame) < Self::BOOTSTRAP_NO_ROOT_FRAMES {
                return;
            }
            let mut ids = connected_peers.to_vec();
            ids.push(local_id);
            Self::sort_peer_ids(&mut ids);
            if ids.first() == Some(&local_id) {
                self.promote_self_to_root(current_frame, "no root seen after bootstrap");
            }
            return;
        }

        if let Some(root) = self.super_root_id {
            match self.root_departure_frame {
                Some(evidence_frame) => {
                    // Only a message from the root *after* the departure
                    // observation disproves it; "seen recently" naturally
                    // includes traffic from just before it died.
                    let spoke_after_evidence = self
                        .last_peer_message_frames
                        .get(&root)
                        .map(|frame| *frame > evidence_frame)
                        .unwrap_or(false);
                    if spoke_after_evidence {
                        self.root_departure_frame = None;
                        return;
                    }
                }
                None => {
                    if self.peer_seen_recently(root, current_frame) {
                        // Live root, no evidence against it: nothing to do.
                        return;
                    }
                }
            }
        }
        let map_age = current_frame.saturating_sub(self.last_map_rx_frame);
        let evidence_age = self
            .root_departure_frame
            .map(|frame| current_frame.saturating_sub(frame));
        // Without positive evidence, only prolonged silence triggers takeover.
        if evidence_age.is_none() && map_age < Self::ROOT_STALE_FRAMES {
            return;
        }
        let root_hash = self.roster_root_hash().unwrap_or(0);
        let mut candidates: Vec<&TopologyEntry> = self
            .topology_roster
            .iter()
            .filter(|entry| entry.peer_hash != root_hash)
            .collect();
        candidates.sort_by(|a, b| a.uuid.cmp(&b.uuid));
        let Some(rank) = candidates
            .iter()
            .position(|entry| entry.peer_hash == local_hash)
        else {
            return;
        };
        let rank = rank as u32;
        let due = if let Some(evidence_age) = evidence_age {
            // Clean departure: take over within seconds, staggered by rank.
            let fast_due = Self::ROOT_DEPARTURE_TAKEOVER_BASE_FRAMES
                .saturating_add(rank.saturating_mul(Self::ROOT_DEPARTURE_TAKEOVER_STAGGER_FRAMES));
            if evidence_age < fast_due {
                return;
            }
            fast_due
        } else {
            let slow_due = Self::ROOT_STALE_FRAMES
                .saturating_add(rank.saturating_mul(Self::ROOT_TAKEOVER_STAGGER_FRAMES));
            if map_age < slow_due {
                return;
            }
            slow_due
        };
        let _ = due;
        self.promote_self_to_root(current_frame, "root presumed dead");
    }

    /// Test/debug: the exact state the failover predicate evaluates.
    pub fn failover_debug(&self) -> String {
        let now = self.last_update_frame;
        let local_hash = self
            .local_peer_hash
            .or(self.local_peer_id.map(Self::peer_hash_for_matchbox));
        let root_hash = self.roster_root_hash().unwrap_or(0);
        let mut candidates: Vec<&TopologyEntry> = self
            .topology_roster
            .iter()
            .filter(|entry| entry.peer_hash != root_hash)
            .collect();
        candidates.sort_by(|a, b| a.uuid.cmp(&b.uuid));
        let rank =
            local_hash.and_then(|hash| candidates.iter().position(|entry| entry.peer_hash == hash));
        let root_msg_age = self
            .super_root_id
            .and_then(|root| self.last_peer_message_frames.get(&root))
            .map(|frame| now.saturating_sub(*frame) as i64)
            .unwrap_or(-1);
        format!(
            "now={now};is_host={};evidence_age={:?};map_age={};epoch={};roster={};rank={:?};root_resolved={};root_msg_age={root_msg_age}",
            self.is_host,
            self.root_departure_frame.map(|f| now.saturating_sub(f)),
            now.saturating_sub(self.last_map_rx_frame),
            self.relay_epoch,
            self.topology_roster.len(),
            rank,
            self.super_root_id.is_some(),
        )
    }

    /// Test/debug: force a specific tree fanout (0 clears the override).
    pub fn set_fanout_override(&mut self, fanout: usize) {
        self.fanout_override = if fanout == 0 { None } else { Some(fanout) };
    }

    /// Voluntary root handoff: a throttled/backgrounded root runs the world
    /// at ~1Hz, which degrades the whole room. Reassign the root slot to
    /// another member through the normal map mechanism and become a regular
    /// member. If the successor is also throttled it hands off again, so the
    /// role cascades to a foreground node.
    pub fn handoff_root(&mut self, current_frame: u32) {
        if !self.is_host || self.topology_roster.len() < 2 {
            return;
        }
        // Settle period: hold the role for a while after acquiring it. With
        // two players alt-tabbing, the role follows the foreground player at
        // a damped rate instead of ping-ponging epochs on every focus change.
        if self.root_acquired_frame != 0
            && current_frame.saturating_sub(self.root_acquired_frame) < 600
        {
            return;
        }
        let Some(local_hash) = self.local_peer_hash else {
            return;
        };
        let successor = self
            .topology_roster
            .iter()
            .filter(|entry| entry.peer_hash != local_hash)
            .min_by(|a, b| a.uuid.cmp(&b.uuid))
            .map(|entry| entry.peer_hash);
        let Some(successor) = successor else {
            return;
        };
        Self::log_warn(&format!(
            "Throttled root handing off to {successor:#x} (epoch {} -> {})",
            self.relay_epoch,
            self.relay_epoch.wrapping_add(1)
        ));
        for entry in self.topology_roster.iter_mut() {
            if entry.peer_hash == successor {
                entry.parent_hash = 0;
            } else if entry.peer_hash == local_hash {
                entry.parent_hash = successor;
            }
        }
        self.member_parent_since.insert(local_hash, current_frame);
        self.member_parent_since.insert(successor, current_frame);
        self.relay_epoch = self.relay_epoch.wrapping_add(1);
        self.last_topology_broadcast_frame = current_frame;
        self.roster_dirty = false;
        // Broadcast while we still hold the role, then derive (which drops
        // our host status since our entry now has a parent).
        self.last_broadcast_roster.clear();
        self.broadcast_topology_map();
        self.last_broadcast_roster = self.topology_roster.clone();
        self.last_broadcast_epoch = self.relay_epoch;
        self.member_last_seen.clear();
        self.member_link_nags.clear();
        self.derive_links_from_roster(current_frame);
    }

    fn promote_self_to_root(&mut self, current_frame: u32, reason: &str) {
        let Some(local_id) = self.local_peer_id else {
            return;
        };
        let local_uuid = Self::uuid_for_peer(local_id);
        let (local_hash, _) = self.register_peer_uuid(local_uuid);
        self.local_peer_hash = Some(local_hash);
        Self::log_warn(&format!(
            "Promoting self to relay root ({reason}); epoch {} -> {}",
            self.relay_epoch,
            self.relay_epoch.wrapping_add(1)
        ));

        let old_root = self.roster_root_hash();
        self.topology_roster
            .retain(|entry| entry.peer_hash != local_hash && Some(entry.peer_hash) != old_root);
        for entry in self.topology_roster.iter_mut() {
            if Some(entry.parent_hash) == old_root || entry.parent_hash == 0 {
                entry.parent_hash = local_hash;
            }
        }
        self.topology_roster.insert(
            0,
            TopologyEntry {
                peer_hash: local_hash,
                uuid: local_uuid,
                parent_hash: 0,
            },
        );
        // Fresh TTL for inherited members so we don't prune the whole room.
        self.member_last_seen.clear();
        let roster_snapshot: Vec<u64> = self
            .topology_roster
            .iter()
            .map(|entry| entry.peer_hash)
            .collect();
        for hash in roster_snapshot {
            self.member_last_seen.insert(hash, current_frame);
            self.member_parent_since.insert(hash, current_frame);
        }
        self.member_link_nags.clear();
        // New epoch lineage: the next broadcast must be a full-map anchor.
        self.last_broadcast_roster.clear();
        self.last_broadcast_epoch = 0;
        self.map_anchor_counter = 0;
        self.is_host = true;
        self.root_acquired_frame = current_frame;
        self.relay_parent = None;
        self.relay_backup_parent = None;
        self.relay_active_parent = None;
        self.relay_epoch = self.relay_epoch.wrapping_add(1);
        self.roster_dirty = true;
        self.last_map_rx_frame = current_frame;
        self.root_departure_frame = None;
    }

    /// Root: assign each active area to the *player nearest to it* — the
    /// node whose local prediction of those enemies is most trustworthy and
    /// most latency-relevant. Sticky: the incumbent keeps an area unless a
    /// challenger is decisively closer, so ownership doesn't flap at borders.
    fn recompute_area_authorities(&mut self, current_frame: u32) {
        if self.topology_roster.is_empty() {
            self.area_authorities.clear();
            return;
        }

        // Candidate members with known positions (self + visible remotes).
        let mut member_positions: Vec<(u64, Vec2)> = Vec::new();
        if let (Some(local_hash), Some(pos)) = (self.local_peer_hash, self.local_last_pos) {
            member_positions.push((local_hash, pos));
        }
        for (peer_id, remote) in &self.remote_players {
            let hash = Self::peer_identity_hash(peer_id);
            if self.roster_contains(hash) {
                member_positions.push((hash, remote.pos));
            }
        }
        if member_positions.is_empty() {
            return;
        }
        // Deterministic order so ties resolve identically everywhere.
        member_positions.sort_by_key(|(hash, _)| *hash);

        let mut area_samples: HashMap<u32, (Vec2, u32)> = HashMap::new();
        for (_, pos) in &member_positions {
            let area_id = Self::area_id_from_pos(*pos);
            if let Some((sum, count)) = area_samples.get_mut(&area_id) {
                *sum += *pos;
                *count = count.saturating_add(1);
            } else {
                area_samples.insert(area_id, (*pos, 1));
            }
        }

        let mut authorities = HashMap::new();
        for (area_id, (sum, count)) in area_samples {
            let center = sum / (count.max(1) as f32);
            let mut best: Option<(u64, f32)> = None;
            for (hash, pos) in &member_positions {
                let d = *pos - center;
                let dist_sq = d.x * d.x + d.y * d.y;
                match best {
                    Some((_, best_score)) if dist_sq >= best_score => {}
                    _ => best = Some((*hash, dist_sq)),
                }
            }
            let Some((winner, winner_dist)) = best else {
                continue;
            };
            // Hysteresis: keep the incumbent unless the winner is decisively
            // closer (or the incumbent is gone / has no known position).
            let chosen =
                match self.area_authorities.get(&area_id) {
                    Some(prev) if *prev != winner => {
                        let prev_dist = member_positions.iter().find(|(hash, _)| hash == prev).map(
                            |(_, pos)| {
                                let d = *pos - center;
                                d.x * d.x + d.y * d.y
                            },
                        );
                        match prev_dist {
                            Some(prev_dist) if winner_dist * 2.25 >= prev_dist => *prev,
                            _ => winner,
                        }
                    }
                    _ => winner,
                };
            authorities.insert(area_id, chosen);
        }

        self.area_authorities = authorities;
        if self.is_host
            && current_frame.saturating_sub(self.last_area_update_broadcast_frame) >= 120
        {
            self.last_area_update_broadcast_frame = current_frame;
            self.broadcast_area_authorities();
        }
    }

    /// Areas whose enemy simulation this node currently owns.
    pub fn owned_area_ids(&self) -> HashSet<u32> {
        let Some(local_hash) = self.local_peer_hash else {
            return HashSet::new();
        };
        self.area_authorities
            .iter()
            .filter(|(_, authority)| **authority == local_hash)
            .map(|(area_id, _)| *area_id)
            .collect()
    }

    /// All areas that have an assigned authority (everything else defaults
    /// to the host).
    pub fn assigned_area_ids(&self) -> HashSet<u32> {
        self.area_authorities.keys().copied().collect()
    }

    /// May `origin` speak authoritatively about enemies in `area_id`?
    /// Assigned areas belong to their authority; unassigned areas belong to
    /// the host.
    fn area_owned_by(&self, area_id: u32, origin_hash: u64) -> bool {
        match self.area_authorities.get(&area_id) {
            Some(authority) => *authority == origin_hash,
            None => {
                let root_hash = self
                    .super_root_id
                    .map(Self::peer_hash_for_matchbox)
                    .unwrap_or(0);
                origin_hash == root_hash
            }
        }
    }

    fn peer_connected_age(
        &self,
        peer_id: matchbox_socket::PeerId,
        current_frame: u32,
    ) -> Option<u32> {
        self.peer_connected_frames
            .get(&peer_id)
            .map(|frame| current_frame.saturating_sub(*frame))
    }

    fn peer_message_age(
        &self,
        peer_id: matchbox_socket::PeerId,
        current_frame: u32,
    ) -> Option<u32> {
        self.last_peer_message_frames
            .get(&peer_id)
            .map(|frame| current_frame.saturating_sub(*frame))
    }

    fn peer_stale_age(&self, peer_id: matchbox_socket::PeerId, current_frame: u32) -> Option<u32> {
        if let Some(age) = self.peer_message_age(peer_id, current_frame) {
            return Some(age);
        }
        let connected_age = self.peer_connected_age(peer_id, current_frame)?;
        if connected_age <= Self::PEER_HANDSHAKE_GRACE_FRAMES {
            None
        } else {
            Some(connected_age)
        }
    }

    fn peer_seen_recently(&self, peer_id: matchbox_socket::PeerId, current_frame: u32) -> bool {
        if let Some(seen_age) = self.peer_message_age(peer_id, current_frame) {
            return seen_age <= Self::RELAY_PARENT_STALE_FRAMES;
        }
        if let Some(connected_age) = self.peer_connected_age(peer_id, current_frame) {
            return connected_age <= Self::PEER_HANDSHAKE_GRACE_FRAMES;
        }
        false
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

        if current_frame.saturating_sub(self.last_parent_switch_frame)
            < Self::RELAY_FAILOVER_COOLDOWN_FRAMES
        {
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

    fn maybe_backoff_silent_peers(
        &mut self,
        current_frame: u32,
        connected_peers: &[matchbox_socket::PeerId],
    ) {
        self.peer_link_backoff_until
            .retain(|_, until| *until > current_frame);

        for peer_id in connected_peers {
            if self.peer_link_backoff_until.contains_key(peer_id) {
                continue;
            }
            let Some(age) = self.peer_stale_age(*peer_id, current_frame) else {
                continue;
            };
            if age < Self::STALE_LINK_RESET_FRAMES {
                continue;
            }
            self.peer_link_backoff_until.insert(
                *peer_id,
                current_frame.saturating_add(Self::STALE_LINK_BACKOFF_FRAMES),
            );
            web_sys::console::warn_1(
                &format!(
                    "Backoff silent peer link {:?} after {} stale frames",
                    peer_id, age
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
            // No parent assigned, or our active route is dead: desire the root
            // directly. Recovery pairs with the root's rescue links below: for
            // any pair, the side that joined signaling first knows the other
            // and can initiate, so one of the two always forms the link.
            if self.relay_parent.is_none() || self.relay_active_parent.is_none() {
                if let Some(root) = self.super_root_id {
                    desired.insert(root);
                }
            }
        }

        // Bootstrap: before topology converges, keep a small fixed-degree neighbor set.
        if desired.is_empty() {
            let mut sorted = known_peers.to_vec();
            Self::sort_peer_ids(&mut sorted);
            if let Some(local) = local_id {
                sorted.retain(|id| *id != local);
            }
            for peer in sorted.into_iter().take(4) {
                optional.push(peer);
            }
        }

        // Admission: root/supernodes desire peers that joined signaling but have
        // no roster slot yet, so newcomers always get an initial offer. Stable
        // (uuid order, no epoch rotation) so links never churn on epoch bumps.
        if self.is_host || self.local_is_supernode() {
            let mut candidates: Vec<matchbox_socket::PeerId> = known_peers
                .iter()
                .copied()
                .filter(|id| Some(*id) != local_id)
                .filter(|id| !desired.contains(id))
                .filter(|id| !self.roster_contains(Self::peer_hash_for_matchbox(*id)))
                .collect();
            Self::sort_peer_ids(&mut candidates);
            let admission_target = if self.is_host { 4 } else { 2 };
            for peer in candidates.into_iter().take(admission_target) {
                optional.push(peer);
            }
        }

        // Rescue: the root reaches out to roster members that went quiet (their
        // relay path likely died) so a stranded subtree regains a route before
        // it gets pruned. Halfway through the TTL is the trigger.
        if self.is_host {
            let connected: HashSet<matchbox_socket::PeerId> =
                connected_peers.iter().copied().collect();
            let mut rescues: Vec<matchbox_socket::PeerId> = self
                .topology_roster
                .iter()
                .filter(|entry| Some(entry.peer_hash) != self.local_peer_hash)
                .filter(|entry| {
                    let last_seen = self
                        .member_last_seen
                        .get(&entry.peer_hash)
                        .copied()
                        .unwrap_or(0);
                    self.last_update_frame.saturating_sub(last_seen) > Self::MEMBER_TTL_FRAMES / 2
                })
                .filter_map(|entry| self.hash_to_matchbox(entry.peer_hash))
                .filter(|id| !connected.contains(id) && !desired.contains(id))
                .collect();
            Self::sort_peer_ids(&mut rescues);
            for peer in rescues.into_iter().take(4) {
                optional.push(peer);
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
        desired.retain(|peer_id| {
            self.peer_link_backoff_until
                .get(peer_id)
                .map(|until| *until <= self.last_update_frame)
                .unwrap_or(true)
        });
        desired
    }

    fn update_desired_peers(
        &mut self,
        known_peers: &[matchbox_socket::PeerId],
        connected_peers: &[matchbox_socket::PeerId],
    ) {
        let desired = self.desired_peer_links(known_peers, connected_peers);
        let current_frame = self.last_update_frame;

        // Make-before-break: a live link that fell out of the desired set is
        // kept through a grace window (replacement tree links form first),
        // then dropped lazily. Never force-drop on a mere desired-set delta.
        let mut to_drop: Vec<matchbox_socket::PeerId> = Vec::new();
        for peer_id in connected_peers {
            if desired.contains(peer_id) {
                self.undesired_since.remove(peer_id);
                continue;
            }
            let since = *self
                .undesired_since
                .entry(*peer_id)
                .or_insert(current_frame);
            if current_frame.saturating_sub(since) >= Self::UNDESIRED_LINK_GRACE_FRAMES {
                to_drop.push(*peer_id);
            }
        }
        let connected_set: HashSet<matchbox_socket::PeerId> =
            connected_peers.iter().copied().collect();
        self.undesired_since
            .retain(|peer_id, _| connected_set.contains(peer_id));

        if let Some(socket) = &mut self.socket {
            for peer_id in to_drop {
                Self::log_info(&format!(
                    "[sync-trace f={current_frame}] dropping link {:?} after undesired grace",
                    peer_id
                ));
                self.undesired_since.remove(&peer_id);
                self.intentional_drops.insert(peer_id, current_frame);
                socket.drop_peer(peer_id);
            }
            if desired != self.desired_peer_set {
                socket.set_desired_peers(desired.iter().copied());
            }
        }
        self.desired_peer_set = desired;
    }

    fn apply_area_authority_update(&mut self, sender: &str, update: AreaAuthorityUpdate) {
        if self.relay_epoch > update.epoch {
            return;
        }
        // The map is root-originated but arrives via the tree: depth >= 2
        // nodes receive it from their parent, not from the root directly.
        if !(self.is_supernode_sender(sender) || self.is_parent_sender(sender)) {
            return;
        }
        self.area_authorities = update
            .entries
            .into_iter()
            .map(|entry| (entry.area_id, entry.authority_hash))
            .collect();
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
        if self.is_host || self.local_is_supernode() {
            return false;
        }
        // During initial bootstrap there may be no authoritative route yet;
        // avoid poisoning supernode selection before any peer traffic is observed.
        if self.last_peer_message_frames.is_empty() {
            return false;
        }
        if current_frame.saturating_sub(self.last_enemy_sync_frame) < 180 {
            return false;
        }
        if let Some(parent) = self.relay_active_parent.or(self.relay_parent) {
            return !self.peer_seen_recently(parent, current_frame);
        }
        true
    }

    /// The authoritative route went quiet. With a sticky root we no longer
    /// poison the root globally; we fail over to the backup parent immediately
    /// and ask the root for a fresh tree slot.
    pub fn mark_supernode_bad(&mut self, current_frame: u32) {
        self.relay_active_parent = None;
        self.last_parent_switch_frame = 0;
        self.last_join_request_frame = 0;
        // Reset the detection window so we don't re-trigger every frame.
        self.last_enemy_sync_frame = current_frame;
    }

    pub fn is_supernode_sender(&self, peer_id: &str) -> bool {
        match (self.supernode_id, self.peer_id_lookup.get(peer_id)) {
            (Some(supernode), Some(sender)) => *sender == supernode,
            _ => false,
        }
    }

    fn is_parent_sender(&self, peer_id: &str) -> bool {
        match (
            self.relay_active_parent.or(self.relay_parent),
            self.peer_id_lookup.get(peer_id),
        ) {
            (Some(parent), Some(sender)) => *sender == parent,
            _ => false,
        }
    }

    fn is_local_peer_str(&self, peer_id: &str) -> bool {
        match self.local_peer_id {
            Some(local) => format!("{:?}", local) == peer_id,
            None => false,
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
        let within_handoff_window = self
            .last_update_frame
            .saturating_sub(self.last_parent_switch_frame)
            <= Self::RELAY_HANDOFF_DUPLEX_FRAMES;
        if within_handoff_window {
            if let Some(primary_target) =
                primary.filter(|target| Some(*target) != self.local_peer_id)
            {
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

    fn bootstrap_accept_sender(&self, peer_id: &str) -> bool {
        if self.is_host {
            return false;
        }
        let sender = match self.peer_id_lookup.get(peer_id) {
            Some(id) => *id,
            None => return false,
        };
        let directly_connected = match &self.socket {
            Some(socket) => socket.connected_peers().any(|id| id == sender),
            None => false,
        };
        if !directly_connected {
            return false;
        }
        let no_route = self.relay_active_parent.or(self.relay_parent).is_none();
        // Include 3-peer LAN parties: bootstrap acceptance relaxes stricter relay admission.
        let tiny_room = self.known_peer_count() <= 3;
        // Allow tiny/no-route bootstrap acceptance even after discovery detach so
        // direct peers don't become "connected but blind" during parent churn.
        self.relay_epoch == 0 || no_route || tiny_room
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
            LowPriorityTopic::Stats => &mut self.last_lowpri_stats_frame,
        };
        if *slot == u32::MAX || frame.saturating_sub(*slot) >= stride {
            *slot = frame;
            true
        } else {
            self.relay_telemetry.dropped_messages =
                self.relay_telemetry.dropped_messages.saturating_add(1);
            false
        }
    }

    fn local_origin_hash(&self) -> Option<u64> {
        self.local_peer_hash
            .or(self.local_peer_id.map(Self::peer_hash_for_matchbox))
    }

    fn should_skip_relay_target(
        target: matchbox_socket::PeerId,
        exclude: Option<matchbox_socket::PeerId>,
        origin_hash: Option<u64>,
    ) -> bool {
        if Some(target) == exclude {
            return true;
        }
        if let Some(origin_hash) = origin_hash {
            if Self::peer_hash_for_matchbox(target) == origin_hash {
                return true;
            }
        }
        false
    }

    fn send_upstream_or_broadcast(&mut self, msg: Vec<u8>) {
        self.send_upstream_or_broadcast_routed(msg, None, self.local_origin_hash());
    }

    fn send_upstream_or_broadcast_routed(
        &mut self,
        msg: Vec<u8>,
        exclude: Option<matchbox_socket::PeerId>,
        origin_hash: Option<u64>,
    ) {
        let targets = self.upstream_targets();
        let payload = origin_hash
            .map(|hash| Self::encode_relay_envelope(hash, &msg))
            .unwrap_or(msg);
        let socket = match &mut self.socket {
            Some(s) => s,
            None => return,
        };
        if !targets.is_empty() {
            for target in targets {
                if Self::should_skip_relay_target(target, exclude, origin_hash) {
                    continue;
                }
                socket.send(payload.clone().into_boxed_slice(), target);
                self.relay_telemetry.sent_upstream =
                    self.relay_telemetry.sent_upstream.saturating_add(1);
            }
            return;
        }
        for peer_id in socket.connected_peers().collect::<Vec<_>>() {
            if Self::should_skip_relay_target(peer_id, exclude, origin_hash) {
                continue;
            }
            socket.send(payload.clone().into_boxed_slice(), peer_id);
            self.relay_telemetry.sent_broadcast =
                self.relay_telemetry.sent_broadcast.saturating_add(1);
        }
    }

    fn send_low_priority_upstream_or_broadcast(&mut self, topic: LowPriorityTopic, msg: Vec<u8>) {
        self.send_low_priority_upstream_or_broadcast_routed(
            topic,
            msg,
            None,
            self.local_origin_hash(),
        );
    }

    fn send_low_priority_upstream_or_broadcast_routed(
        &mut self,
        topic: LowPriorityTopic,
        msg: Vec<u8>,
        exclude: Option<matchbox_socket::PeerId>,
        origin_hash: Option<u64>,
    ) {
        if self.allow_low_priority_topic(topic) {
            self.send_upstream_or_broadcast_routed(msg, exclude, origin_hash);
        }
    }

    fn send_downstream_or_broadcast(
        &mut self,
        msg: Vec<u8>,
        exclude: Option<matchbox_socket::PeerId>,
    ) {
        self.send_downstream_or_broadcast_routed(msg, exclude, self.local_origin_hash());
    }

    fn send_downstream_or_broadcast_routed(
        &mut self,
        msg: Vec<u8>,
        exclude: Option<matchbox_socket::PeerId>,
        origin_hash: Option<u64>,
    ) {
        let payload = origin_hash
            .map(|hash| Self::encode_relay_envelope(hash, &msg))
            .unwrap_or(msg);
        let socket = match &mut self.socket {
            Some(s) => s,
            None => return,
        };
        if self.relay_children.is_empty() {
            for peer_id in socket.connected_peers().collect::<Vec<_>>() {
                if Self::should_skip_relay_target(peer_id, exclude, origin_hash) {
                    continue;
                }
                socket.send(payload.clone().into_boxed_slice(), peer_id);
                self.relay_telemetry.sent_broadcast =
                    self.relay_telemetry.sent_broadcast.saturating_add(1);
            }
            return;
        }
        for peer_id in &self.relay_children {
            if Self::should_skip_relay_target(*peer_id, exclude, origin_hash) {
                continue;
            }
            socket.send(payload.clone().into_boxed_slice(), *peer_id);
            self.relay_telemetry.sent_downstream =
                self.relay_telemetry.sent_downstream.saturating_add(1);
        }
    }

    fn send_low_priority_downstream_or_broadcast(
        &mut self,
        topic: LowPriorityTopic,
        msg: Vec<u8>,
        exclude: Option<matchbox_socket::PeerId>,
    ) {
        self.send_low_priority_downstream_or_broadcast_routed(
            topic,
            msg,
            exclude,
            self.local_origin_hash(),
        );
    }

    fn send_low_priority_downstream_or_broadcast_routed(
        &mut self,
        topic: LowPriorityTopic,
        msg: Vec<u8>,
        exclude: Option<matchbox_socket::PeerId>,
        origin_hash: Option<u64>,
    ) {
        if self.allow_low_priority_topic(topic) {
            self.send_downstream_or_broadcast_routed(msg, exclude, origin_hash);
        }
    }

    /// Tree flood for control-plane events (names, etc.): forward to every
    /// tree neighbor except the link the message arrived on. From a child the
    /// message goes up AND to the sibling subtrees; from the parent it goes
    /// down. The origin-hash envelope prevents echoes to the originator.
    fn relay_control_message(
        &mut self,
        sender: &str,
        msg: Vec<u8>,
        relay_origin_hash: Option<u64>,
    ) {
        let sender_matchbox = self.peer_id_lookup.get(sender).copied();
        let from_parent = self.is_parent_sender(sender);
        let from_child = self.is_child_sender(sender);
        let origin_hash = relay_origin_hash.unwrap_or_else(|| Self::hash_peer_id(sender));

        if from_parent {
            if !self.relay_children.is_empty() {
                self.send_downstream_or_broadcast_routed(msg, sender_matchbox, Some(origin_hash));
            }
            return;
        }

        if from_child {
            if !self.is_host && self.relay_parent_or_root().is_some() {
                self.send_upstream_or_broadcast_routed(
                    msg.clone(),
                    sender_matchbox,
                    Some(origin_hash),
                );
            }
            if !self.relay_children.is_empty() {
                self.send_downstream_or_broadcast_routed(msg, sender_matchbox, Some(origin_hash));
            }
            return;
        }

        if self.is_host {
            self.send_downstream_or_broadcast_routed(msg, sender_matchbox, Some(origin_hash));
        } else {
            self.send_upstream_or_broadcast_routed(msg, sender_matchbox, Some(origin_hash));
        }
    }

    fn relay_low_priority_control_message(
        &mut self,
        sender: &str,
        topic: LowPriorityTopic,
        msg: Vec<u8>,
        relay_origin_hash: Option<u64>,
    ) {
        // Consume the topic budget once for the whole flood step, then reuse
        // the plain tree-flood (up + sibling subtrees, minus the inbound link).
        if !self.allow_low_priority_topic(topic) {
            return;
        }
        self.relay_control_message(sender, msg, relay_origin_hash);
    }

    pub fn record_paid_obstacle_confirmation(
        &mut self,
        proof_hash: [u8; 32],
        peer_id: matchbox_socket::PeerId,
    ) -> usize {
        let entry = self
            .paid_obstacle_confirmations
            .entry(proof_hash)
            .or_default();
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

    pub fn record_paid_ability_confirmation(
        &mut self,
        proof_hash: [u8; 32],
        peer_id: matchbox_socket::PeerId,
    ) -> usize {
        let entry = self
            .paid_ability_confirmations
            .entry(proof_hash)
            .or_default();
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

    pub fn record_paid_name_confirmation(
        &mut self,
        proof_hash: [u8; 32],
        peer_id: matchbox_socket::PeerId,
    ) -> usize {
        let entry = self.paid_name_confirmations.entry(proof_hash).or_default();
        entry.insert(peer_id);
        entry.len()
    }

    pub fn paid_name_confirmation_count(&self, proof_hash: [u8; 32]) -> usize {
        self.paid_name_confirmations
            .get(&proof_hash)
            .map(|set| set.len())
            .unwrap_or(0)
    }

    pub fn paid_name_has_supernode_ack(&self, proof_hash: [u8; 32]) -> bool {
        let supernode = match self.supernode_id {
            Some(id) => id,
            None => return true,
        };
        self.paid_name_confirmations
            .get(&proof_hash)
            .map(|set| set.contains(&supernode))
            .unwrap_or(false)
    }

    pub fn resolve_peer_id(&self, peer_id: &str) -> Option<matchbox_socket::PeerId> {
        self.peer_id_lookup.get(peer_id).copied()
    }

    pub fn resolve_peer_hash(&self, hash: u64) -> Option<PeerId> {
        self.identity_peer_id_for_hash(hash)
    }

    fn synthetic_peer_id_for_hash(hash: u64) -> PeerId {
        format!("OverlayPeer({hash:016x})")
    }

    fn parse_synthetic_peer_hash(peer_id: &str) -> Option<u64> {
        let hex = peer_id.strip_prefix("OverlayPeer(")?.strip_suffix(')')?;
        u64::from_str_radix(hex, 16).ok()
    }

    fn peer_identity_hash(peer_id: &str) -> u64 {
        Self::parse_synthetic_peer_hash(peer_id).unwrap_or_else(|| Self::hash_peer_id(peer_id))
    }

    fn migrate_peer_alias(&mut self, from: &str, to: &str) {
        if from == to {
            return;
        }
        if let Some(remote) = self.remote_players.remove(from) {
            self.remote_players.entry(to.to_string()).or_insert(remote);
        }
        if let Some(stats) = self.remote_stats.remove(from) {
            self.remote_stats.entry(to.to_string()).or_insert(stats);
        }
        if let Some(name) = self.pending_player_names.remove(from) {
            self.pending_player_names
                .entry(to.to_string())
                .or_insert(name);
        }
        for (peer_id, _) in self.pending_input_frames.iter_mut() {
            if peer_id == from {
                *peer_id = to.to_string();
            }
        }
    }

    fn resolve_or_register_peer_hash(&mut self, hash: u64) -> PeerId {
        if let Some(peer_id) = self.peer_identity_lookup.get(&hash) {
            return peer_id.clone();
        }
        let canonical = self
            .peer_hash_lookup
            .get(&hash)
            .cloned()
            .unwrap_or_else(|| Self::synthetic_peer_id_for_hash(hash));
        self.peer_identity_lookup.insert(hash, canonical.clone());
        canonical
    }

    fn peer_id_key(peer_id: matchbox_socket::PeerId) -> String {
        format!("{:?}", peer_id)
    }

    fn peer_id_ordering(
        a: matchbox_socket::PeerId,
        b: matchbox_socket::PeerId,
    ) -> std::cmp::Ordering {
        Self::peer_id_key(a).cmp(&Self::peer_id_key(b))
    }

    fn sort_peer_ids(ids: &mut Vec<matchbox_socket::PeerId>) {
        ids.sort_by(|a, b| Self::peer_id_ordering(*a, *b));
    }

    fn cap_discovery_peers(
        local_id: Option<matchbox_socket::PeerId>,
        connected_peers: &[matchbox_socket::PeerId],
        known_peers: &mut Vec<matchbox_socket::PeerId>,
    ) {
        if known_peers.len() <= Self::MAX_DISCOVERY_PEERS {
            return;
        }

        let local_hash = local_id.map(Self::peer_hash_for_matchbox).unwrap_or(0);
        known_peers.sort_by_key(|peer_id| Self::peer_hash_for_matchbox(*peer_id) ^ local_hash);
        known_peers.dedup();

        let mut retained = Vec::new();
        let mut seen = HashSet::new();
        for peer_id in connected_peers {
            if seen.insert(*peer_id) {
                retained.push(*peer_id);
            }
        }
        for peer_id in known_peers.iter().copied() {
            if retained.len() >= Self::MAX_DISCOVERY_PEERS {
                break;
            }
            if seen.insert(peer_id) {
                retained.push(peer_id);
            }
        }

        Self::sort_peer_ids(&mut retained);
        retained.dedup();
        *known_peers = retained;
    }

    fn peer_hash_for_matchbox(peer_id: matchbox_socket::PeerId) -> u64 {
        Self::hash_peer_id(&format!("{:?}", peer_id))
    }

    fn encode_relay_envelope(origin_hash: u64, payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(Self::RELAY_ENVELOPE_MAGIC.len() + 8 + payload.len());
        out.extend_from_slice(&Self::RELAY_ENVELOPE_MAGIC);
        out.extend_from_slice(&origin_hash.to_le_bytes());
        out.extend_from_slice(payload);
        out
    }

    fn decode_relay_envelope(bytes: &[u8]) -> Option<(u64, Vec<u8>)> {
        let header_len = Self::RELAY_ENVELOPE_MAGIC.len();
        if bytes.len() < header_len + 8 || bytes[..header_len] != Self::RELAY_ENVELOPE_MAGIC {
            return None;
        }
        let origin_hash = u64::from_le_bytes([
            bytes[header_len],
            bytes[header_len + 1],
            bytes[header_len + 2],
            bytes[header_len + 3],
            bytes[header_len + 4],
            bytes[header_len + 5],
            bytes[header_len + 6],
            bytes[header_len + 7],
        ]);
        Some((origin_hash, bytes[(header_len + 8)..].to_vec()))
    }

    fn hash_peer_id(peer_id: &str) -> u64 {
        let mut hash: u64 = 0xcbf29ce484222325;
        for b in peer_id.as_bytes() {
            hash ^= *b as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash
    }

    pub fn normalize_player_name(name: &str) -> String {
        let normalized: String = name
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(20)
            .map(|c| c.to_ascii_uppercase())
            .collect();
        if normalized.is_empty() {
            "PLAYER".to_string()
        } else {
            normalized
        }
    }

    fn sanitize_display_base(name: &str) -> String {
        Self::normalize_player_name(name)
    }

    fn display_name_map_by_hash(&self) -> HashMap<u64, String> {
        let mut base_by_hash: HashMap<u64, String> = HashMap::new();
        let mut groups: HashMap<String, Vec<u64>> = HashMap::new();

        if let Some(local_hash) = self.local_peer_hash {
            let base = Self::sanitize_display_base(&self.local_player_name);
            base_by_hash.insert(local_hash, base.clone());
            groups
                .entry(base.to_ascii_lowercase())
                .or_default()
                .push(local_hash);
        }

        for (peer_id, remote) in &self.remote_players {
            let hash = Self::peer_identity_hash(peer_id);
            let base = Self::sanitize_display_base(&remote.name);
            base_by_hash.insert(hash, base.clone());
            groups
                .entry(base.to_ascii_lowercase())
                .or_default()
                .push(hash);
        }

        let mut display_by_hash: HashMap<u64, String> = HashMap::new();
        for (base_key, hashes) in groups.iter_mut() {
            hashes.sort_unstable();
            hashes.dedup();
            let reserved_owner = self
                .name_reservations
                .get(base_key)
                .map(|reservation| reservation.owner_hash);

            if let Some(owner_hash) = reserved_owner {
                let base = self
                    .name_reservations
                    .get(base_key)
                    .map(|reservation| Self::sanitize_display_base(&reservation.name_string()))
                    .unwrap_or_else(|| {
                        hashes
                            .first()
                            .and_then(|hash| base_by_hash.get(hash))
                            .cloned()
                            .unwrap_or_else(|| "PLAYER".to_string())
                    });
                let owner_display_hash = if let Some(local_hash) = self.local_peer_hash {
                    if self.local_name_owner_hash() == owner_hash && hashes.contains(&local_hash) {
                        Some(local_hash)
                    } else {
                        None
                    }
                } else {
                    None
                };
                if let Some(owner_display_hash) = owner_display_hash {
                    display_by_hash.insert(owner_display_hash, base.clone());
                    let mut others: Vec<u64> = hashes
                        .iter()
                        .copied()
                        .filter(|hash| *hash != owner_display_hash)
                        .collect();
                    others.sort_unstable();
                    for (idx, hash) in others.iter().enumerate() {
                        display_by_hash.insert(*hash, format!("{base}#{}", idx + 2));
                    }
                } else if hashes.len() == 1 {
                    display_by_hash.insert(hashes[0], base.clone());
                } else {
                    for (idx, hash) in hashes.iter().enumerate() {
                        display_by_hash.insert(*hash, format!("{base}#{}", idx + 1));
                    }
                }
                continue;
            }

            let multi = hashes.len() > 1;
            for (idx, hash) in hashes.iter().enumerate() {
                let base = base_by_hash
                    .get(hash)
                    .cloned()
                    .unwrap_or_else(|| "PLAYER".to_string());
                let display = if multi {
                    format!("{base}#{}", idx + 1)
                } else {
                    base
                };
                display_by_hash.insert(*hash, display);
            }
        }

        display_by_hash
    }

    fn tick_latency(&mut self, current_frame: u32, connected_peers: &[matchbox_socket::PeerId]) {
        let socket = match &mut self.socket {
            Some(s) => s,
            None => return,
        };

        if current_frame.saturating_sub(self.last_ping_frame) >= 120 {
            self.last_ping_frame = current_frame;
            let now_ms = js_sys::Date::now() as u64 as u32;
            let ping = Ping {
                nonce: current_frame,
                sent_ms: now_ms,
            };
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
                let score_ms: u32 = connected_peers
                    .iter()
                    .map(|peer_id| *self.latency_ms.get(peer_id).unwrap_or(&1000))
                    .sum();
                let score = SupernodeScore {
                    score_ms,
                    sample_count,
                };
                let msg = NetMessage::SupernodeScoreEvent(score).to_bytes();
                for peer_id in connected_peers {
                    socket.send(msg.clone().into_boxed_slice(), *peer_id);
                }
            }
        }
    }

    fn network_state_label(&self) -> &'static str {
        match self.state {
            NetworkState::Disconnected => "disconnected",
            NetworkState::Connecting => "connecting",
            NetworkState::WaitingForPeers => "waiting",
            NetworkState::Connected => "connected",
            NetworkState::Error(_) => "error",
        }
    }

    fn peer_list_signature(peers: &[matchbox_socket::PeerId]) -> u64 {
        let mut sorted = peers.to_vec();
        Self::sort_peer_ids(&mut sorted);
        let mut sig: u64 = 0xcbf29ce484222325;
        for peer_id in sorted {
            let id_sig = Self::peer_hash_for_matchbox(peer_id);
            sig ^= id_sig;
            sig = sig.wrapping_mul(0x100000001b3);
        }
        sig
    }

    fn peer_list_compact(peers: &[matchbox_socket::PeerId]) -> String {
        if peers.is_empty() {
            return "none".to_string();
        }
        let mut sorted = peers.to_vec();
        Self::sort_peer_ids(&mut sorted);
        let mut labels = Vec::new();
        for peer_id in sorted.into_iter().take(8) {
            labels.push(format!("{:?}", peer_id));
        }
        if peers.len() > 8 {
            labels.push(format!("+{}", peers.len() - 8));
        }
        labels.join(",")
    }

    fn sync_trace_signature(
        &self,
        connected_peers: &[matchbox_socket::PeerId],
        known_peers: &[matchbox_socket::PeerId],
    ) -> u64 {
        let mut sig: u64 = 0xcbf29ce484222325;
        let parts = [
            Self::peer_list_signature(connected_peers),
            Self::peer_list_signature(known_peers),
            self.desired_peer_set.len() as u64,
            self.remote_players.len() as u64,
            self.relay_epoch as u64,
            if self.discovery_attached { 1 } else { 0 },
            if self.is_host { 1 } else { 0 },
            self.relay_parent
                .map(Self::peer_hash_for_matchbox)
                .unwrap_or(0),
            self.relay_backup_parent
                .map(Self::peer_hash_for_matchbox)
                .unwrap_or(0),
            self.relay_active_parent
                .map(Self::peer_hash_for_matchbox)
                .unwrap_or(0),
            self.super_root_id
                .map(Self::peer_hash_for_matchbox)
                .unwrap_or(0),
            self.supernode_id
                .map(Self::peer_hash_for_matchbox)
                .unwrap_or(0),
        ];
        for value in parts {
            sig ^= value;
            sig = sig.wrapping_mul(0x100000001b3);
        }
        sig
    }

    fn log_sync_trace(
        &mut self,
        current_frame: u32,
        connected_peers: &[matchbox_socket::PeerId],
        known_peers: &[matchbox_socket::PeerId],
    ) {
        let sig = self.sync_trace_signature(connected_peers, known_peers);
        let periodic = current_frame.saturating_sub(self.last_sync_trace_frame)
            >= Self::SYNC_TRACE_PERIOD_FRAMES;
        let changed = sig != self.last_sync_trace_sig;
        if changed || periodic {
            self.last_sync_trace_sig = sig;
            self.last_sync_trace_frame = current_frame;
            let mut stale: Vec<String> = Vec::new();
            let mut worst_age = 0u32;
            let mut newest_peer = None;
            for peer_id in connected_peers {
                let age = self.peer_stale_age(*peer_id, current_frame);
                if let Some(age) = age {
                    if age >= Self::SYNC_STALE_WARN_FRAMES {
                        stale.push(format!("{:?}:{age}", peer_id));
                    }
                    if age > worst_age {
                        worst_age = age;
                        newest_peer = Some(*peer_id);
                    }
                }
            }
            let stale_label = if stale.is_empty() {
                "none".to_string()
            } else {
                stale.join(",")
            };
            let worst_peer = newest_peer
                .map(|id| format!("{:?}", id))
                .unwrap_or_else(|| "none".to_string());
            web_sys::console::log_1(
                &format!(
                    "[sync-trace f={current_frame}] state={} room={} conn={} known={} desired={} remote={} discovery={} epoch={} host={} parent={:?} active={:?} backup={:?} root={:?} super={:?} worst_age={} worst_peer={} stale={} conn_ids=[{}] known_ids=[{}]",
                    self.network_state_label(),
                    self.room_code,
                    connected_peers.len(),
                    known_peers.len(),
                    self.desired_peer_set.len(),
                    self.remote_players.len(),
                    self.discovery_attached,
                    self.relay_epoch,
                    self.is_host,
                    self.relay_parent,
                    self.relay_active_parent,
                    self.relay_backup_parent,
                    self.super_root_id,
                    self.supernode_id,
                    worst_age,
                    worst_peer,
                    stale_label,
                    Self::peer_list_compact(connected_peers),
                    Self::peer_list_compact(known_peers),
                )
                .into(),
            );
        }

        if current_frame.saturating_sub(self.last_sync_warn_frame) < Self::SYNC_TRACE_PERIOD_FRAMES
        {
            return;
        }

        for peer_id in connected_peers {
            let Some(age) = self.peer_stale_age(*peer_id, current_frame) else {
                continue;
            };
            if age >= Self::SYNC_STALE_WARN_FRAMES {
                self.last_sync_warn_frame = current_frame;
                web_sys::console::warn_1(
                    &format!(
                        "[sync-trace f={current_frame}] peer silence detected: peer={:?} age={} conn={} known={} desired={} discovery={} epoch={} parent={:?} active={:?}",
                        peer_id,
                        age,
                        connected_peers.len(),
                        known_peers.len(),
                        self.desired_peer_set.len(),
                        self.discovery_attached,
                        self.relay_epoch,
                        self.relay_parent,
                        self.relay_active_parent,
                    )
                    .into(),
                );
                break;
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
            let pong = Pong {
                nonce: ping.nonce,
                sent_ms: ping.sent_ms,
            };
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

    pub fn send_player_stats_snapshot(&mut self) {
        let Some(player_hash) = self.local_origin_hash() else {
            return;
        };
        if player_hash == 0 {
            return;
        }

        let snapshot = self.snapshot_from_stats(player_hash, &self.local_stats);
        let msg = NetMessage::PlayerStatsEvent(snapshot).to_bytes();
        if self.is_host {
            self.send_low_priority_downstream_or_broadcast(LowPriorityTopic::Stats, msg, None);
        } else {
            self.send_low_priority_upstream_or_broadcast(LowPriorityTopic::Stats, msg);
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
        if vote.target_hash == 0 || vote.voter_hash == 0 || vote.target_hash == vote.voter_hash {
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
        let display_by_hash = self.display_name_map_by_hash();

        let mut unique_matches: Vec<u64> = Vec::new();
        let mut base_matches: Vec<u64> = Vec::new();

        if let Some(local_hash) = self.local_peer_hash {
            let local_base = Self::sanitize_display_base(&self.local_player_name);
            let local_unique = display_by_hash
                .get(&local_hash)
                .cloned()
                .unwrap_or(local_base.clone());
            if local_unique.to_ascii_lowercase() == query {
                unique_matches.push(local_hash);
            }
            if local_base.to_ascii_lowercase() == query {
                base_matches.push(local_hash);
            }
        }

        for (peer_id, remote) in &self.remote_players {
            let hash = Self::peer_identity_hash(peer_id);
            let base = Self::sanitize_display_base(&remote.name);
            let unique = display_by_hash.get(&hash).cloned().unwrap_or(base.clone());
            if unique.to_ascii_lowercase() == query {
                unique_matches.push(hash);
            }
            if base.to_ascii_lowercase() == query {
                base_matches.push(hash);
            }
        }

        unique_matches.sort_unstable();
        unique_matches.dedup();
        if unique_matches.len() == 1 {
            return unique_matches.first().copied();
        }

        base_matches.sort_unstable();
        base_matches.dedup();
        if base_matches.len() == 1 {
            return base_matches.first().copied();
        }

        None
    }

    pub fn matching_display_names(&self, name: &str) -> Vec<String> {
        let query = name.trim().to_ascii_lowercase();
        if query.is_empty() {
            return Vec::new();
        }
        let display_by_hash = self.display_name_map_by_hash();

        let mut matches = Vec::new();
        if let Some(local_hash) = self.local_peer_hash {
            let local_base = Self::sanitize_display_base(&self.local_player_name);
            let local_unique = display_by_hash
                .get(&local_hash)
                .cloned()
                .unwrap_or(local_base.clone());
            if local_base.to_ascii_lowercase() == query
                || local_unique.to_ascii_lowercase() == query
            {
                matches.push(local_unique);
            }
        }

        for (peer_id, remote) in &self.remote_players {
            let hash = Self::peer_identity_hash(peer_id);
            let base = Self::sanitize_display_base(&remote.name);
            let unique = display_by_hash.get(&hash).cloned().unwrap_or(base.clone());
            if base.to_ascii_lowercase() == query || unique.to_ascii_lowercase() == query {
                matches.push(unique);
            }
        }

        matches.sort_by(|a, b| a.to_ascii_lowercase().cmp(&b.to_ascii_lowercase()));
        matches.dedup();
        matches
    }

    pub fn reserved_name_owner_hash(&self, name: &str) -> Option<u64> {
        let key = Self::normalize_player_name(name).to_ascii_lowercase();
        self.name_reservations
            .get(&key)
            .map(|reservation| reservation.owner_hash)
    }

    pub fn is_name_reserved_by_other(&self, name: &str, local_hash: u64) -> bool {
        self.reserved_name_owner_hash(name)
            .map(|owner| owner != local_hash)
            .unwrap_or(false)
    }

    pub fn is_name_reserved_by_self(&self, name: &str, local_hash: u64) -> bool {
        self.reserved_name_owner_hash(name)
            .map(|owner| owner == local_hash)
            .unwrap_or(false)
    }

    pub fn apply_paid_name_reservation(&mut self, reservation: PaidNameReservation) -> bool {
        let normalized_name = Self::normalize_player_name(&reservation.name_string());
        let key = normalized_name.to_ascii_lowercase();
        if let Some(existing) = self.name_reservations.get(&key) {
            if existing.owner_hash != reservation.owner_hash {
                return false;
            }
            if existing.nonce >= reservation.nonce {
                return false;
            }
        }
        let normalized = PaidNameReservation::from_name(
            reservation.owner_hash,
            &normalized_name,
            reservation.nonce,
            reservation.proof_hash,
        );
        self.name_reservations.insert(key, normalized);
        true
    }

    pub fn store_paid_name_candidate(&mut self, reservation: PaidNameReservation) {
        self.pending_paid_name_candidates
            .insert(reservation.proof_hash, reservation);
    }

    pub fn take_paid_name_candidate(
        &mut self,
        proof_hash: [u8; 32],
    ) -> Option<PaidNameReservation> {
        self.pending_paid_name_candidates.remove(&proof_hash)
    }

    pub fn pending_paid_name_hashes(&self) -> Vec<[u8; 32]> {
        self.pending_paid_name_candidates.keys().copied().collect()
    }

    pub fn paid_name_reservations_snapshot(&self) -> Vec<PaidNameReservation> {
        let mut reservations: Vec<PaidNameReservation> =
            self.name_reservations.values().copied().collect();
        reservations.sort_by(|a, b| a.name_string().cmp(&b.name_string()));
        reservations
    }

    pub fn ensure_local_name_not_reserved_by_other(&mut self) -> Option<String> {
        let local_hash = self.local_name_owner_hash();
        if local_hash == 0 {
            return None;
        }
        let normalized = Self::normalize_player_name(&self.local_player_name);
        if !self.is_name_reserved_by_other(&normalized, local_hash) {
            return None;
        }
        let suffix = (local_hash % 1000) as u16;
        let mut fallback = format!("{normalized}{suffix:03}");
        if fallback.len() > 20 {
            let cut = 20usize.saturating_sub(3);
            fallback = format!("{}{:03}", &normalized[..cut.min(normalized.len())], suffix);
        }
        self.local_player_name = fallback.clone();
        Some(fallback)
    }

    pub fn local_display_name(&self) -> String {
        let base = Self::sanitize_display_base(&self.local_player_name);
        let hash = match self.local_peer_hash {
            Some(hash) => hash,
            None => return base,
        };
        self.display_name_map_by_hash()
            .get(&hash)
            .cloned()
            .unwrap_or(base)
    }

    pub fn display_names_snapshot(&self) -> (String, HashMap<PeerId, String>) {
        let display_by_hash = self.display_name_map_by_hash();
        let local_name = if let Some(hash) = self.local_peer_hash {
            display_by_hash
                .get(&hash)
                .cloned()
                .unwrap_or_else(|| Self::sanitize_display_base(&self.local_player_name))
        } else {
            Self::sanitize_display_base(&self.local_player_name)
        };
        let mut remote_names = HashMap::new();
        for (peer_id, remote) in &self.remote_players {
            let hash = Self::peer_identity_hash(peer_id);
            let base = Self::sanitize_display_base(&remote.name);
            let display = display_by_hash.get(&hash).cloned().unwrap_or(base);
            remote_names.insert(peer_id.clone(), display);
        }
        (local_name, remote_names)
    }

    pub fn display_name_for_peer_id(&self, peer_id: &str) -> String {
        let hash = Self::peer_identity_hash(peer_id);
        let canonical_peer_id = self
            .identity_peer_id_for_hash(hash)
            .unwrap_or_else(|| peer_id.to_string());
        if let Some(remote) = self.remote_players.get(&canonical_peer_id) {
            let base = Self::sanitize_display_base(&remote.name);
            return self
                .display_name_map_by_hash()
                .get(&hash)
                .cloned()
                .unwrap_or(base);
        }
        "Player".to_string()
    }

    pub fn display_name_for_hash(&self, hash: u64) -> String {
        self.display_name_map_by_hash()
            .get(&hash)
            .cloned()
            .unwrap_or_else(|| "Player".to_string())
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
            self.local_stats.attack_attempts =
                self.local_stats.attack_attempts.saturating_add(count);
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

    fn snapshot_from_stats(&self, player_hash: u64, stats: &PlayerStats) -> PlayerStatsSnapshot {
        PlayerStatsSnapshot {
            player_hash,
            kills: stats.kills,
            spider_kills: stats.spider_kills,
            cannon_kills: stats.cannon_kills,
            snake_kills: stats.snake_kills,
            wisp_kills: stats.wisp_kills,
            attack_attempts: stats.attack_attempts,
            attack_hits: stats.attack_hits,
            deaths: stats.deaths,
            time_played_frames: stats.time_played_frames,
        }
    }

    fn stats_from_snapshot(snapshot: PlayerStatsSnapshot) -> PlayerStats {
        PlayerStats {
            kills: snapshot.kills,
            spider_kills: snapshot.spider_kills,
            cannon_kills: snapshot.cannon_kills,
            snake_kills: snapshot.snake_kills,
            wisp_kills: snapshot.wisp_kills,
            attack_attempts: snapshot.attack_attempts,
            attack_hits: snapshot.attack_hits,
            deaths: snapshot.deaths,
            time_played_frames: snapshot.time_played_frames,
        }
    }

    fn apply_remote_stats_snapshot(&mut self, snapshot: PlayerStatsSnapshot) {
        let peer_id = self.resolve_or_register_peer_hash(snapshot.player_hash);
        if self.is_local_peer_str(&peer_id) {
            return;
        }

        let incoming = Self::stats_from_snapshot(snapshot);
        let stats = self.remote_stats.entry(peer_id).or_default();
        if incoming.time_played_frames >= stats.time_played_frames {
            *stats = incoming;
        }
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
            totals.time_played_frames = totals
                .time_played_frames
                .saturating_add(stats.time_played_frames);
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
        if let Some(existing) = self.relay_player_states.get(&peer_hash) {
            if existing.sim_frame() >= state.sim_frame() {
                return;
            }
        }
        self.relay_player_states.insert(peer_hash, state);
    }

    fn queue_relay_input_frame(&mut self, peer_hash: u64, frame: InputFrame) {
        let area_id = self.area_id_for_hash(peer_hash);
        if self.relay_input_frames.len() >= Self::MAX_RELAY_INPUT_QUEUE {
            self.relay_input_frames.remove(0);
            self.relay_telemetry.dropped_queue_entries =
                self.relay_telemetry.dropped_queue_entries.saturating_add(1);
        }
        self.relay_input_frames.push(InputFrameEntry {
            peer_hash,
            area_id,
            frame,
        });
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
        // Periodically bypass interest filtering for player-state batches so
        // far-away players still reach everyone at a low rate (roster, player
        // list, minimap). Cadence widens with room size to bound the cost.
        self.flush_counter = self.flush_counter.wrapping_add(1);
        let bypass_stride: u32 = if self.topology_roster.len() <= 128 {
            8
        } else {
            32
        };
        let bypass_interest = self.flush_counter % bypass_stride == 0;
        let identity_lookup = self.peer_identity_lookup.clone();
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
                let _ = identity_lookup.get(&local_hash);
            }
        }

        let mut id_lookup: HashMap<matchbox_socket::PeerId, PeerId> = HashMap::new();
        for (peer_id, matchbox_id) in &self.peer_id_lookup {
            id_lookup.insert(*matchbox_id, peer_id.clone());
        }

        let queue_depth = self.relay_player_states.len()
            + self.relay_input_frames.len()
            + self
                .downlink_player_batches
                .iter()
                .map(|b| b.entries.len())
                .sum::<usize>()
            + self
                .downlink_input_batches
                .iter()
                .map(|b| b.entries.len())
                .sum::<usize>();
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
                    PlayerStateEntry {
                        peer_hash,
                        area_id,
                        state,
                    }
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
                let msg =
                    NetMessage::PlayerStateBatchEvent(PlayerStateBatch { entries }).to_bytes();
                socket.send(msg.into_boxed_slice(), parent_peer);
            } else if !children.is_empty() {
                for child in &children {
                    let child_id = match id_lookup.get(child) {
                        Some(id) => id.clone(),
                        None => continue,
                    };
                    let target_hash = Self::hash_peer_id(&child_id);
                    let target_key = identity_lookup
                        .get(&target_hash)
                        .cloned()
                        .unwrap_or_else(|| child_id.clone());
                    let target_pos = remote_positions.get(&target_key).copied();
                    let target_area = target_pos.map(Self::area_id_from_pos);
                    let mut filtered: Vec<PlayerStateEntry> = entries
                        .iter()
                        .filter(|entry| {
                            if bypass_interest || entry.peer_hash == target_hash {
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
                    let msg =
                        NetMessage::PlayerStateBatchEvent(PlayerStateBatch { entries: filtered })
                            .to_bytes();
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
                    let target_key = identity_lookup
                        .get(&target_hash)
                        .cloned()
                        .unwrap_or_else(|| child_id.clone());
                    let target_pos = remote_positions.get(&target_key).copied();
                    let target_area = target_pos.map(Self::area_id_from_pos);
                    let mut filtered: Vec<PlayerStateEntry> = batch
                        .entries
                        .iter()
                        .filter(|entry| {
                            if bypass_interest || entry.peer_hash == target_hash {
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
                    let msg =
                        NetMessage::PlayerStateBatchEvent(PlayerStateBatch { entries: filtered })
                            .to_bytes();
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
                    let target_key = identity_lookup
                        .get(&target_hash)
                        .cloned()
                        .unwrap_or_else(|| child_id.clone());
                    let target_pos = remote_positions.get(&target_key).copied();
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
                            let entry_peer_id = match identity_lookup.get(&entry.peer_hash) {
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
                    let msg =
                        NetMessage::InputFrameBatchEvent(InputFrameBatch { entries: filtered })
                            .to_bytes();
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
                    let target_key = identity_lookup
                        .get(&target_hash)
                        .cloned()
                        .unwrap_or_else(|| child_id.clone());
                    let target_pos = remote_positions.get(&target_key).copied();
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
                            let entry_peer_id = match identity_lookup.get(&entry.peer_hash) {
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
                    let msg =
                        NetMessage::InputFrameBatchEvent(InputFrameBatch { entries: filtered })
                            .to_bytes();
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
        let candidate_target = self
            .area_authority_for(area_id)
            .or(self.relay_parent_or_root())
            .filter(|target| Some(*target) != self.local_peer_id);
        let socket = match &mut self.socket {
            Some(s) => s,
            None => return,
        };
        let connected_peers: Vec<_> = socket.connected_peers().collect();
        let target = candidate_target
            .filter(|peer_id| connected_peers.iter().any(|connected| connected == peer_id));
        let msg = NetMessage::PlayerUpdate(state).to_bytes();
        if let Some(target) = target {
            socket.send(msg.into_boxed_slice(), target);
        } else {
            for peer_id in connected_peers {
                socket.send(msg.clone().into_boxed_slice(), peer_id);
            }
        }
    }

    pub fn apply_predicted_states(
        &mut self,
        predictions: &std::collections::HashMap<String, PlayerState>,
    ) {
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
            + self
                .downlink_player_batches
                .iter()
                .map(|b| b.entries.len())
                .sum::<usize>()
            + self
                .downlink_input_batches
                .iter()
                .map(|b| b.entries.len())
                .sum::<usize>()
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

    pub fn desired_peer_debug(&self) -> String {
        let mut ids: Vec<String> = self
            .desired_peer_set
            .iter()
            .map(|id| format!("{:?}", id))
            .collect();
        ids.sort();
        if ids.is_empty() {
            "none".to_string()
        } else {
            ids.join(",")
        }
    }

    pub fn discovery_attached(&self) -> bool {
        self.discovery_attached
    }

    pub fn relay_epoch(&self) -> u32 {
        self.relay_epoch
    }

    /// Send enemy sync to all peers (host only)
    /// Send an enemy-sync correction for the areas this node owns (the host
    /// additionally covers unassigned areas). Floods through the tree from
    /// wherever the authority sits.
    pub fn send_enemy_sync(&mut self, sync: EnemySync) {
        let msg = NetMessage::EnemySync(sync).to_bytes();
        let origin = self.local_origin_hash();
        if !self.is_host {
            self.send_upstream_or_broadcast_routed(msg.clone(), None, origin);
        }
        if self.is_host || !self.relay_children.is_empty() {
            self.send_downstream_or_broadcast_routed(msg, None, origin);
        }
    }

    fn relay_enemy_kill(
        &mut self,
        kill: EnemyKill,
        exclude: Option<matchbox_socket::PeerId>,
        origin_hash: Option<u64>,
    ) {
        let msg = NetMessage::EnemyKillEvent(kill).to_bytes();
        self.send_downstream_or_broadcast_routed(msg, exclude, origin_hash);
    }

    fn relay_player_death(
        &mut self,
        death: PlayerDeath,
        exclude: Option<matchbox_socket::PeerId>,
        origin_hash: Option<u64>,
    ) {
        let msg = NetMessage::PlayerDeathEvent(death).to_bytes();
        self.send_downstream_or_broadcast_routed(msg, exclude, origin_hash);
    }

    fn relay_paid_obstacle(
        &mut self,
        obstacle: PaidObstacle,
        exclude: Option<matchbox_socket::PeerId>,
        origin_hash: Option<u64>,
    ) {
        let msg = NetMessage::PaidObstacleEvent(obstacle).to_bytes();
        self.send_downstream_or_broadcast_routed(msg, exclude, origin_hash);
    }

    fn relay_paid_ability(
        &mut self,
        ability: PaidAbility,
        exclude: Option<matchbox_socket::PeerId>,
        origin_hash: Option<u64>,
    ) {
        let msg = NetMessage::PaidAbilityEvent(ability).to_bytes();
        self.send_downstream_or_broadcast_routed(msg, exclude, origin_hash);
    }

    fn relay_paid_name(
        &mut self,
        reservation: PaidNameReservation,
        exclude: Option<matchbox_socket::PeerId>,
        origin_hash: Option<u64>,
    ) {
        let msg = NetMessage::PaidNameReservationEvent(reservation).to_bytes();
        self.send_downstream_or_broadcast_routed(msg, exclude, origin_hash);
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
    pub fn take_enemy_syncs(&mut self) -> Vec<PendingEnemySync> {
        std::mem::take(&mut self.pending_enemy_syncs)
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

    fn forward_wave_start_down(
        &mut self,
        wave_start: WaveStart,
        exclude: Option<matchbox_socket::PeerId>,
        origin_hash: Option<u64>,
    ) {
        if self.relay_children.is_empty() {
            return;
        }
        let msg = NetMessage::WaveStartEvent(wave_start).to_bytes();
        self.send_downstream_or_broadcast_routed(msg, exclude, origin_hash);
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

    /// Send enemy kill events as a single batched message.
    pub fn send_enemy_kills(&mut self, kills: Vec<EnemyKill>) {
        if kills.is_empty() {
            return;
        }
        if kills.len() == 1 {
            self.send_enemy_kill(kills[0]);
            return;
        }
        let msg = NetMessage::EnemyKillBatchEvent(EnemyKillBatch { kills }).to_bytes();
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

    pub fn send_paid_name_reservation(&mut self, reservation: PaidNameReservation) {
        let msg = NetMessage::PaidNameReservationEvent(reservation).to_bytes();
        if self.is_host {
            self.send_downstream_or_broadcast(msg, None);
        } else {
            self.send_upstream_or_broadcast(msg);
        }
    }

    pub fn send_paid_name_reservation_to_supernode(&mut self, reservation: PaidNameReservation) {
        self.send_paid_name_reservation(reservation);
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

    pub fn send_paid_name_ack(&mut self, ack: PaidNameAck) {
        let msg = NetMessage::PaidNameAckEvent(ack).to_bytes();
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

    pub fn send_projectile_reflection(&mut self, reflection: ProjectileReflection) {
        let msg = NetMessage::ProjectileReflectionEvent(reflection).to_bytes();
        if self.is_host {
            self.send_downstream_or_broadcast(msg, None);
        } else {
            self.send_upstream_or_broadcast(msg);
        }
    }

    pub fn send_input_frame(&mut self, frame: InputFrame) {
        if self.is_host {
            if let Some(hash) = self.local_peer_hash {
                self.queue_relay_input_frame(hash, frame);
            }
            return;
        }

        let area_id = self.local_last_pos.map(Self::area_id_from_pos).unwrap_or(0);
        let candidate_target = self
            .area_authority_for(area_id)
            .or(self.relay_parent_or_root())
            .filter(|target| Some(*target) != self.local_peer_id);
        let socket = match &mut self.socket {
            Some(s) => s,
            None => return,
        };
        let connected_peers: Vec<_> = socket.connected_peers().collect();
        let target = candidate_target
            .filter(|peer_id| connected_peers.iter().any(|connected| connected == peer_id));
        let msg = NetMessage::InputFrameEvent(frame).to_bytes();
        if let Some(target) = target {
            socket.send(msg.into_boxed_slice(), target);
        } else {
            for peer_id in connected_peers {
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

    fn handle_enemy_kill(
        &mut self,
        peer_id: PeerId,
        origin_hash: u64,
        kill: EnemyKill,
        current_frame: u32,
    ) {
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
                self.relay_enemy_kill(kill, self.resolve_peer_id(&peer_id), Some(origin_hash));
            }
            return;
        }
        if !self.is_host && from_child {
            self.send_upstream_or_broadcast_routed(
                NetMessage::EnemyKillEvent(kill).to_bytes(),
                self.resolve_peer_id(&peer_id),
                Some(origin_hash),
            );
        }
        let entry = self
            .enemy_kill_confirmations
            .entry(kill.event_id)
            .or_insert_with(|| (kill, HashSet::new(), current_frame));
        entry.1.insert(peer_id.clone());

        if self.is_host {
            if entry.1.len() == 1 {
                self.relay_enemy_kill(kill, self.resolve_peer_id(&peer_id), Some(origin_hash));
            }
            self.pending_enemy_kills_confirmed.push(kill);
            self.applied_event_ids.insert(kill.event_id, current_frame);
            self.enemy_kill_confirmations.remove(&kill.event_id);
            return;
        }

        if !self.optimistic_enemy_event_ids.contains_key(&kill.event_id) {
            self.pending_enemy_kills_optimistic.push(kill);
            self.optimistic_enemy_event_ids
                .insert(kill.event_id, current_frame);
        }

        if entry.1.len() >= 2 {
            let event = entry.0;
            self.pending_enemy_kills_confirmed.push(event);
            self.applied_event_ids.insert(event.event_id, current_frame);
            self.optimistic_enemy_event_ids.remove(&event.event_id);
            self.enemy_kill_confirmations.remove(&event.event_id);
        }
    }

    fn handle_player_death(
        &mut self,
        peer_id: PeerId,
        origin_hash: u64,
        death: PlayerDeath,
        current_frame: u32,
    ) {
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
                self.relay_player_death(death, self.resolve_peer_id(&peer_id), Some(origin_hash));
            }
            return;
        }
        if !self.is_host && from_child {
            self.send_upstream_or_broadcast_routed(
                NetMessage::PlayerDeathEvent(death).to_bytes(),
                self.resolve_peer_id(&peer_id),
                Some(origin_hash),
            );
        }
        let entry = self
            .player_death_confirmations
            .entry(death.event_id)
            .or_insert_with(|| (death, HashSet::new(), current_frame));
        entry.1.insert(peer_id.clone());

        if self.is_host {
            if entry.1.len() == 1 {
                self.relay_player_death(death, self.resolve_peer_id(&peer_id), Some(origin_hash));
            }
            self.pending_player_deaths_confirmed.push(death);
            self.applied_event_ids.insert(death.event_id, current_frame);
            self.player_death_confirmations.remove(&death.event_id);
            return;
        }

        if !self
            .optimistic_death_event_ids
            .contains_key(&death.event_id)
        {
            self.pending_player_deaths_optimistic.push(death);
            self.optimistic_death_event_ids
                .insert(death.event_id, current_frame);
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

    pub fn take_paid_names(&mut self) -> Vec<(PeerId, PaidNameReservation)> {
        std::mem::take(&mut self.pending_paid_names)
    }

    pub fn take_paid_name_acks(&mut self) -> Vec<(PeerId, PaidNameAck)> {
        std::mem::take(&mut self.pending_paid_name_acks)
    }

    pub fn take_cannon_shots(&mut self) -> Vec<CannonShot> {
        std::mem::take(&mut self.pending_cannon_shots)
    }

    pub fn take_projectile_reflections(&mut self) -> Vec<ProjectileReflection> {
        std::mem::take(&mut self.pending_projectile_reflections)
    }

    pub fn take_input_frames(&mut self) -> Vec<(PeerId, InputFrame)> {
        std::mem::take(&mut self.pending_input_frames)
    }

    /// Send wave start to specific peers (for late joiners)
    pub fn send_wave_start_to_peers(
        &mut self,
        wave_start: &WaveStart,
        peers: &[matchbox_socket::PeerId],
    ) {
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
            web_sys::console::log_1(
                &format!("Sent wave state to late joiner: {:?}", peer_id).into(),
            );
        }
    }

    /// Send paid obstacle sync to specific peers (for late joiners)
    pub fn send_paid_obstacles_to_peers(
        &mut self,
        obstacles: &[PaidObstacle],
        peers: &[matchbox_socket::PeerId],
    ) {
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
            web_sys::console::log_1(
                &format!("Sent paid obstacles to late joiner: {:?}", peer_id).into(),
            );
        }
    }

    pub fn send_paid_obstacles_to_all(&mut self, obstacles: &[PaidObstacle]) {
        if !self.is_host || obstacles.is_empty() {
            return;
        }

        let sync = PaidObstacleSync {
            obstacles: obstacles.to_vec(),
        };
        let msg = NetMessage::PaidObstacleSyncEvent(sync).to_bytes();
        self.send_downstream_or_broadcast(msg, None);
    }

    pub fn send_paid_names_to_peers(
        &mut self,
        reservations: &[PaidNameReservation],
        peers: &[matchbox_socket::PeerId],
    ) {
        if !self.is_host || peers.is_empty() || reservations.is_empty() {
            return;
        }

        let socket = match &mut self.socket {
            Some(s) => s,
            None => return,
        };

        let sync = PaidNameSync {
            reservations: reservations.to_vec(),
        };
        let msg = NetMessage::PaidNameSyncEvent(sync).to_bytes();

        for peer_id in peers {
            socket.send(msg.clone().into_boxed_slice(), *peer_id);
            web_sys::console::log_1(
                &format!("Sent paid names to late joiner: {:?}", peer_id).into(),
            );
        }
    }

    pub fn send_paid_names_to_all(&mut self, reservations: &[PaidNameReservation]) {
        if !self.is_host || reservations.is_empty() {
            return;
        }

        let sync = PaidNameSync {
            reservations: reservations.to_vec(),
        };
        let msg = NetMessage::PaidNameSyncEvent(sync).to_bytes();
        self.send_downstream_or_broadcast(msg, None);
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
        let proof_hash =
            Self::compute_paid_ability_hash(&self.room_code, ability_type, x, y, radius, nonce);
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

    pub fn build_paid_name_reservation(
        &self,
        owner_hash: u64,
        name: &str,
        nonce: u32,
    ) -> PaidNameReservation {
        let normalized = Self::normalize_player_name(name);
        let proof_hash = Self::compute_paid_name_hash(&normalized, owner_hash, nonce);
        PaidNameReservation::from_name(owner_hash, &normalized, nonce, proof_hash)
    }

    pub fn verify_paid_name_reservation(&self, reservation: &PaidNameReservation) -> bool {
        if reservation.owner_hash == 0 {
            return false;
        }
        let normalized = Self::normalize_player_name(&reservation.name_string());
        let expected =
            Self::compute_paid_name_hash(&normalized, reservation.owner_hash, reservation.nonce);
        reservation.proof_hash == expected
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

    fn compute_paid_name_hash(normalized_name: &str, owner_hash: u64, nonce: u32) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(b"slime-name-reservation-v1");
        hasher.update(normalized_name.as_bytes());
        hasher.update(&owner_hash.to_le_bytes());
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

            let loops = if size >= 10_000 {
                20
            } else if size >= 1_000 {
                60
            } else {
                200
            };
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

    #[test]
    fn duplicate_names_are_disambiguated_and_resolvable() {
        let mut session = NetworkSession::new();
        let local = peer_from(0);
        let remote_a = peer_from(1);
        let remote_b = peer_from(2);

        session.local_peer_id = Some(local);
        let local_peer_id_str = format!("{:?}", local);
        let local_hash = NetworkSession::hash_peer_id(&local_peer_id_str);
        session.local_peer_hash = Some(local_hash);
        session.local_player_name = "KEY".to_string();

        let state = PlayerState::new(
            1,
            Vec2::new(0.0, 0.0),
            Vec2::RIGHT,
            Vec2::RIGHT,
            true,
            false,
            false,
            false,
            false,
        );

        let remote_a_id = format!("{:?}", remote_a);
        let remote_b_id = format!("{:?}", remote_b);
        session.remote_players.insert(
            remote_a_id.clone(),
            RemotePlayer::new("KEY".to_string(), &state, 1),
        );
        session.remote_players.insert(
            remote_b_id.clone(),
            RemotePlayer::new("KEY".to_string(), &state, 1),
        );

        let remote_a_hash = NetworkSession::hash_peer_id(&remote_a_id);
        let remote_b_hash = NetworkSession::hash_peer_id(&remote_b_id);
        session.peer_id_lookup.insert(remote_a_id.clone(), remote_a);
        session.peer_id_lookup.insert(remote_b_id.clone(), remote_b);
        session
            .peer_hash_lookup
            .insert(remote_a_hash, remote_a_id.clone());
        session
            .peer_hash_lookup
            .insert(remote_b_hash, remote_b_id.clone());

        let local_name = session.local_display_name();
        let a_name = session.display_name_for_peer_id(&remote_a_id);
        let b_name = session.display_name_for_peer_id(&remote_b_id);

        assert!(local_name.starts_with("KEY#"));
        assert!(a_name.starts_with("KEY#"));
        assert!(b_name.starts_with("KEY#"));
        assert_ne!(local_name, a_name);
        assert_ne!(local_name, b_name);
        assert_ne!(a_name, b_name);

        assert_eq!(session.resolve_hash_by_name("KEY"), None);
        assert_eq!(session.resolve_hash_by_name(&local_name), Some(local_hash));
        assert_eq!(session.resolve_hash_by_name(&a_name), Some(remote_a_hash));
        assert_eq!(session.resolve_hash_by_name(&b_name), Some(remote_b_hash));
    }

    #[test]
    fn reserved_name_keeps_owner_unsuffixed() {
        let mut session = NetworkSession::new();
        let local = peer_from(0);
        let remote = peer_from(1);

        session.local_peer_id = Some(local);
        let local_peer_id_str = format!("{:?}", local);
        let local_hash = NetworkSession::hash_peer_id(&local_peer_id_str);
        session.local_peer_hash = Some(local_hash);
        session.local_player_name = "KEY".to_string();

        let state = PlayerState::new(
            1,
            Vec2::new(0.0, 0.0),
            Vec2::RIGHT,
            Vec2::RIGHT,
            true,
            false,
            false,
            false,
            false,
        );
        let remote_id = format!("{:?}", remote);
        session.remote_players.insert(
            remote_id.clone(),
            RemotePlayer::new("KEY".to_string(), &state, 1),
        );

        let reservation = session.build_paid_name_reservation(local_hash, "KEY", 10);
        assert!(session.verify_paid_name_reservation(&reservation));
        assert!(session.apply_paid_name_reservation(reservation));

        assert_eq!(session.local_display_name(), "KEY".to_string());
        let remote_display = session.display_name_for_peer_id(&remote_id);
        assert!(remote_display.starts_with("KEY#"));
    }

    #[test]
    fn reserved_name_enforcement_uses_local_owner_identity() {
        let mut blocked_session = NetworkSession::new();
        blocked_session.local_player_name = "KEY".to_string();
        blocked_session.set_local_name_owner_hash(777);

        let reservation_other_owner = blocked_session.build_paid_name_reservation(42, "KEY", 1);
        assert!(blocked_session.apply_paid_name_reservation(reservation_other_owner));
        let fallback = blocked_session.ensure_local_name_not_reserved_by_other();
        assert!(fallback.is_some());

        let mut owner_session = NetworkSession::new();
        owner_session.local_player_name = "KEY".to_string();
        owner_session.set_local_name_owner_hash(777);
        let reservation_local_owner = owner_session.build_paid_name_reservation(777, "KEY", 1);
        assert!(owner_session.apply_paid_name_reservation(reservation_local_owner));
        let fallback = owner_session.ensure_local_name_not_reserved_by_other();
        assert!(fallback.is_none());
    }

    #[test]
    fn synthetic_peer_identity_resolves_consistently() {
        let mut session = NetworkSession::new();
        let remote = peer_from(7);
        let remote_id = format!("{:?}", remote);
        let remote_hash = NetworkSession::hash_peer_id(&remote_id);
        let synthetic = NetworkSession::synthetic_peer_id_for_hash(remote_hash);
        let state = PlayerState::new(
            8,
            Vec2::new(320.0, -144.0),
            Vec2::LEFT,
            Vec2::LEFT,
            true,
            false,
            true,
            false,
            true,
        );

        session.peer_id_lookup.insert(remote_id.clone(), remote);
        session
            .peer_hash_lookup
            .insert(remote_hash, remote_id.clone());
        session
            .peer_identity_lookup
            .insert(remote_hash, synthetic.clone());
        session.remote_players.insert(
            synthetic.clone(),
            RemotePlayer::new("SYNC".to_string(), &state, 8),
        );
        session.remote_stats.insert(
            synthetic.clone(),
            PlayerStats {
                kills: 3,
                ..PlayerStats::default()
            },
        );

        assert_eq!(
            session.resolve_peer_hash(remote_hash),
            Some(synthetic.clone())
        );
        assert_eq!(
            session.display_name_for_peer_id(&remote_id),
            "SYNC".to_string()
        );
        assert_eq!(
            session.display_name_for_hash(remote_hash),
            "SYNC".to_string()
        );
        assert_eq!(session.resolve_hash_by_name("SYNC"), Some(remote_hash));
        assert_eq!(session.peer_pos(remote), Some(Vec2::new(320.0, -144.0)));
    }

    #[test]
    fn migrate_peer_alias_merges_direct_and_overlay_state() {
        let mut session = NetworkSession::new();
        let remote = peer_from(8);
        let remote_id = format!("{:?}", remote);
        let remote_hash = NetworkSession::hash_peer_id(&remote_id);
        let synthetic = NetworkSession::synthetic_peer_id_for_hash(remote_hash);
        let state = PlayerState::new(
            12,
            Vec2::new(12.0, 24.0),
            Vec2::RIGHT,
            Vec2::RIGHT,
            true,
            false,
            false,
            false,
            false,
        );

        session.peer_id_lookup.insert(remote_id.clone(), remote);
        session
            .peer_hash_lookup
            .insert(remote_hash, remote_id.clone());
        session
            .peer_identity_lookup
            .insert(remote_hash, synthetic.clone());
        session
            .pending_player_names
            .insert(remote_id.clone(), "SYNC".to_string());
        session.remote_players.insert(
            remote_id.clone(),
            RemotePlayer::new("SYNC".to_string(), &state, 12),
        );
        session.remote_stats.insert(
            remote_id.clone(),
            PlayerStats {
                kills: 5,
                ..PlayerStats::default()
            },
        );
        session.pending_input_frames.push((
            remote_id.clone(),
            InputFrame {
                frame: 12,
                input: 0,
            },
        ));

        session.migrate_peer_alias(&remote_id, &synthetic);

        assert!(session.remote_players.contains_key(&synthetic));
        assert!(!session.remote_players.contains_key(&remote_id));
        assert!(session.remote_stats.contains_key(&synthetic));
        assert!(!session.remote_stats.contains_key(&remote_id));
        assert_eq!(
            session
                .pending_input_frames
                .first()
                .map(|(id, _)| id.clone()),
            Some(synthetic)
        );
    }

    fn hash_for(peer: matchbox_socket::PeerId) -> u64 {
        NetworkSession::hash_peer_id(&format!("{:?}", peer))
    }

    fn entry_for(peer: matchbox_socket::PeerId, parent: u64) -> TopologyEntry {
        TopologyEntry {
            peer_hash: hash_for(peer),
            uuid: NetworkSession::uuid_for_peer(peer),
            parent_hash: parent,
        }
    }

    #[test]
    fn root_admits_members_with_sticky_parents() {
        let root = peer_from(0);
        let a = peer_from(1);
        let b = peer_from(2);

        let mut session = NetworkSession::new();
        session.is_host = true;
        session.local_peer_id = Some(root);

        session.update_topology_as_root(10, &[a]);
        let a_parent = session
            .roster_entry(hash_for(a))
            .map(|entry| entry.parent_hash);
        assert_eq!(a_parent, Some(hash_for(root)));
        assert_eq!(session.roster_root_hash(), Some(hash_for(root)));
        let epoch_after_a = session.relay_epoch;

        // Another join must not move A's slot.
        session.update_topology_as_root(20, &[a, b]);
        assert_eq!(
            session
                .roster_entry(hash_for(a))
                .map(|entry| entry.parent_hash),
            a_parent
        );
        assert!(session.roster_contains(hash_for(b)));
        assert!(session.relay_epoch > epoch_after_a);

        // A frame with no membership change must not bump the epoch.
        let stable_epoch = session.relay_epoch;
        session.update_topology_as_root(30, &[a, b]);
        assert_eq!(session.relay_epoch, stable_epoch);
        assert!(session.is_host);
    }

    #[test]
    fn joiner_adopts_map_and_never_self_elects() {
        let root = peer_from(0);
        let mid = peer_from(1);
        let leaf = peer_from(2);

        let mut session = NetworkSession::new();
        session.local_peer_id = Some(mid);
        session.local_peer_hash = Some(hash_for(mid));

        let map = TopologyUpdate {
            epoch: 3,
            root_hash: hash_for(root),
            fanout: 2,
            entries: vec![
                entry_for(root, 0),
                entry_for(mid, hash_for(root)),
                entry_for(leaf, hash_for(mid)),
            ],
        };
        assert!(session.apply_topology_map("sender", &map, 60));

        assert!(!session.is_host);
        assert_eq!(session.super_root_id, Some(root));
        assert_eq!(session.relay_parent, Some(root));
        assert_eq!(session.relay_children, vec![leaf]);
        assert_eq!(session.relay_epoch, 3);

        // A stale map (older epoch, same root) must be rejected.
        let stale = TopologyUpdate {
            epoch: 2,
            root_hash: hash_for(root),
            fanout: 2,
            entries: vec![entry_for(root, 0), entry_for(mid, hash_for(root))],
        };
        assert!(!session.apply_topology_map("sender", &stale, 61));
        assert_eq!(session.relay_epoch, 3);
    }

    #[test]
    fn root_failover_promotes_candidates_in_uuid_order_with_stagger() {
        let root = peer_from(0);
        let first = peer_from(1);
        let second = peer_from(2);

        let map_entries = vec![
            entry_for(root, 0),
            entry_for(first, hash_for(root)),
            entry_for(second, hash_for(root)),
        ];
        let candidate_rank = |peer: matchbox_socket::PeerId| {
            let mut candidates = vec![
                NetworkSession::uuid_for_peer(first),
                NetworkSession::uuid_for_peer(second),
            ];
            candidates.sort();
            candidates
                .iter()
                .position(|uuid| *uuid == NetworkSession::uuid_for_peer(peer))
                .unwrap() as u32
        };

        for peer in [first, second] {
            let mut session = NetworkSession::new();
            session.local_peer_id = Some(peer);
            session.local_peer_hash = Some(hash_for(peer));
            session.topology_roster = map_entries.clone();
            session.relay_epoch = 5;
            session.last_map_rx_frame = 100;

            let rank = candidate_rank(peer);
            let not_yet = 100
                + NetworkSession::ROOT_STALE_FRAMES
                + rank * NetworkSession::ROOT_TAKEOVER_STAGGER_FRAMES
                - 1;
            session.maybe_root_failover(not_yet, &[]);
            assert!(!session.is_host, "rank {rank} promoted too early");

            let due = not_yet + 1;
            session.maybe_root_failover(due, &[]);
            assert!(session.is_host, "rank {rank} failed to promote when due");
            assert_eq!(session.roster_root_hash(), Some(hash_for(peer)));
            assert!(session.relay_epoch > 5);
            // The dead root is gone; the other candidate is re-homed under us.
            assert!(!session.roster_contains(hash_for(root)));
        }
    }

    #[test]
    fn root_scales_to_two_thousand_members_with_bounded_tree() {
        let root = peer_from(0);
        let mut session = NetworkSession::new();
        session.is_host = true;
        session.local_peer_id = Some(root);
        session.local_peer_hash = Some(hash_for(root));
        session.topology_roster.push(entry_for(root, 0));

        const N: usize = 2_000;
        for i in 1..=N {
            // Fanout grows with the room, as update_topology_as_root does.
            session.relay_fanout =
                NetworkSession::choose_dynamic_fanout(session.topology_roster.len());
            session.add_roster_member(NetworkSession::uuid_for_peer(peer_from(i as u128)), 10);
        }
        assert_eq!(session.topology_roster.len(), N + 1);

        // Structure: bounded fanout, no orphans, bounded depth, acyclic.
        let parents: HashMap<u64, u64> = session
            .topology_roster
            .iter()
            .map(|e| (e.peer_hash, e.parent_hash))
            .collect();
        let mut child_counts: HashMap<u64, usize> = HashMap::new();
        for entry in &session.topology_roster {
            if entry.parent_hash != 0 {
                assert!(
                    parents.contains_key(&entry.parent_hash),
                    "orphaned member: parent not in roster"
                );
                *child_counts.entry(entry.parent_hash).or_insert(0) += 1;
            }
        }
        let max_children = child_counts.values().copied().max().unwrap_or(0);
        assert!(
            max_children <= NetworkSession::MAX_FANOUT,
            "fanout exceeded: {max_children}"
        );
        let mut max_depth = 0usize;
        for entry in &session.topology_roster {
            let mut depth = 0;
            let mut cursor = entry.peer_hash;
            while let Some(parent) = parents.get(&cursor).copied() {
                if parent == 0 {
                    break;
                }
                depth += 1;
                cursor = parent;
                assert!(depth <= 64, "cycle or absurd depth in tree");
            }
            max_depth = max_depth.max(depth);
        }
        assert!(max_depth <= 5, "tree too deep for 2k members: {max_depth}");

        // Wire sizes: full map is ~32B/member; a single-join delta is tiny.
        let full = session.topology_map_message().expect("map");
        let full_bytes = NetMessage::TopologyUpdateEvent(full).to_bytes().len();
        assert!(
            full_bytes > 60_000,
            "expected ~64KB full map, got {full_bytes}"
        );
        let delta = TopologyDelta {
            epoch_from: 1,
            epoch_to: 2,
            root_hash: hash_for(root),
            fanout: session.relay_fanout as u8,
            checksum: NetworkSession::roster_checksum(&session.topology_roster),
            removed: vec![],
            upserts: vec![*session.topology_roster.last().unwrap()],
        };
        let delta_bytes = NetMessage::TopologyDeltaEvent(delta).to_bytes().len();
        assert!(
            delta_bytes < 100,
            "single-join delta should be tiny: {delta_bytes}"
        );
    }

    #[test]
    fn delta_applies_with_checksum_and_rejects_desync() {
        let root = peer_from(0);
        let a = peer_from(1);
        let b = peer_from(2);
        let c = peer_from(3);

        let mut session = NetworkSession::new();
        session.local_peer_id = Some(a);
        session.local_peer_hash = Some(hash_for(a));
        let base = TopologyUpdate {
            epoch: 5,
            root_hash: hash_for(root),
            fanout: 4,
            entries: vec![
                entry_for(root, 0),
                entry_for(a, hash_for(root)),
                entry_for(b, hash_for(root)),
            ],
        };
        assert!(session.apply_topology_map("sender", &base, 10));
        assert_eq!(session.relay_epoch, 5);

        // Valid delta: c joins under a.
        let mut next = base.entries.clone();
        next.push(entry_for(c, hash_for(a)));
        let delta = TopologyDelta {
            epoch_from: 5,
            epoch_to: 6,
            root_hash: hash_for(root),
            fanout: 4,
            checksum: NetworkSession::roster_checksum(&next),
            removed: vec![],
            upserts: vec![entry_for(c, hash_for(a))],
        };
        assert!(session.apply_topology_delta("sender", &delta, 20));
        assert_eq!(session.relay_epoch, 6);
        assert!(session.roster_contains(hash_for(c)));
        assert_eq!(session.relay_children, vec![c]);
        assert!(!session.needs_full_map);

        // Replay of the same delta: stale, ignored, no desync flag.
        assert!(!session.apply_topology_delta("sender", &delta, 21));
        assert!(!session.needs_full_map);

        // Gap in the epoch stream: must request a full map.
        let gap = TopologyDelta {
            epoch_from: 8,
            epoch_to: 9,
            root_hash: hash_for(root),
            fanout: 4,
            checksum: 0,
            removed: vec![],
            upserts: vec![],
        };
        assert!(!session.apply_topology_delta("sender", &gap, 22));
        assert!(session.needs_full_map);
        session.needs_full_map = false;

        // Corrupt checksum: rejected, roster untouched, full map requested.
        let bad = TopologyDelta {
            epoch_from: 6,
            epoch_to: 7,
            root_hash: hash_for(root),
            fanout: 4,
            checksum: 0xDEAD,
            removed: vec![hash_for(b)],
            upserts: vec![],
        };
        assert!(!session.apply_topology_delta("sender", &bad, 23));
        assert!(session.needs_full_map);
        assert!(session.roster_contains(hash_for(b)));
        assert_eq!(session.relay_epoch, 6);
    }

    #[test]
    fn heartbeat_delta_refreshes_liveness_only_from_parent() {
        let root = peer_from(0);
        let a = peer_from(1);
        let stranger = peer_from(9);

        let mut session = NetworkSession::new();
        session.local_peer_id = Some(a);
        session.local_peer_hash = Some(hash_for(a));
        let base = TopologyUpdate {
            epoch: 3,
            root_hash: hash_for(root),
            fanout: 4,
            entries: vec![entry_for(root, 0), entry_for(a, hash_for(root))],
        };
        assert!(session.apply_topology_map("sender", &base, 10));
        session.last_map_rx_frame = 10;
        // Resolve sender strings to matchbox ids for authority checks.
        let root_str = format!("{:?}", root);
        let stranger_str = format!("{:?}", stranger);
        session.peer_id_lookup.insert(root_str.clone(), root);
        session
            .peer_id_lookup
            .insert(stranger_str.clone(), stranger);

        let heartbeat = TopologyDelta {
            epoch_from: 3,
            epoch_to: 3,
            root_hash: hash_for(root),
            fanout: 4,
            checksum: NetworkSession::roster_checksum(&session.topology_roster),
            removed: vec![],
            upserts: vec![],
        };
        // From a stranger: not liveness, not forwarded.
        assert!(!session.apply_topology_delta(&stranger_str, &heartbeat, 500));
        assert_eq!(session.last_map_rx_frame, 10);
        // From our parent (the root): refreshes liveness and forwards.
        assert!(session.apply_topology_delta(&root_str, &heartbeat, 600));
        assert_eq!(session.last_map_rx_frame, 600);
        // Divergent checksum from parent: triggers a full-map request.
        let bad_heartbeat = TopologyDelta {
            checksum: 0xBAD,
            ..heartbeat
        };
        assert!(!session.apply_topology_delta(&root_str, &bad_heartbeat, 700));
        assert!(session.needs_full_map);
    }

    #[test]
    fn repeated_join_nags_reassign_unreachable_parent() {
        let root = peer_from(0);
        let parent_a = peer_from(1);
        let parent_b = peer_from(2);
        let leaf = peer_from(3);

        let mut session = NetworkSession::new();
        session.is_host = true;
        session.local_peer_id = Some(root);
        session.local_peer_hash = Some(hash_for(root));
        session.relay_fanout = 4;
        session.topology_roster = vec![
            entry_for(root, 0),
            entry_for(parent_a, hash_for(root)),
            entry_for(parent_b, hash_for(root)),
            entry_for(leaf, hash_for(parent_a)),
        ];
        session.member_parent_since.insert(hash_for(leaf), 0);

        let request = JoinRequest {
            peer_hash: hash_for(leaf),
            uuid: NetworkSession::uuid_for_peer(leaf),
        };
        // First nag at a mature parent age: tracked but below threshold.
        session.handle_join_request("sender", request, 700);
        assert_eq!(
            session.roster_entry(hash_for(leaf)).unwrap().parent_hash,
            hash_for(parent_a)
        );
        // Second nag within the window: reassigned away from parent_a.
        session.handle_join_request("sender", request, 790);
        let new_parent = session.roster_entry(hash_for(leaf)).unwrap().parent_hash;
        assert_ne!(new_parent, hash_for(parent_a));
        assert!(session.roster_dirty);
        // Cooldown: immediate further nags do not bounce it again.
        let parent_after = new_parent;
        session.handle_join_request("sender", request, 850);
        session.handle_join_request("sender", request, 900);
        assert_eq!(
            session.roster_entry(hash_for(leaf)).unwrap().parent_hash,
            parent_after
        );
    }

    #[test]
    fn area_authority_assigns_nearest_member_and_unassigned_defaults_to_host() {
        let root = peer_from(0);
        let remote = peer_from(1);

        let mut session = NetworkSession::new();
        session.is_host = true;
        session.local_peer_id = Some(root);
        session.local_peer_hash = Some(hash_for(root));
        session.super_root_id = Some(root);
        session.topology_roster = vec![entry_for(root, 0), entry_for(remote, hash_for(root))];

        // Root at origin, remote far away in another area.
        session.local_last_pos = Some(Vec2::new(0.0, 0.0));
        let far = Vec2::new(100_000.0, 0.0);
        let near_area = NetworkSession::area_id_for_pos(Vec2::new(0.0, 0.0));
        let far_area = NetworkSession::area_id_for_pos(far);
        assert_ne!(near_area, far_area, "test needs two distinct areas");

        let remote_id = format!("{:?}", remote);
        let state = PlayerState::new(
            1,
            far,
            Vec2::RIGHT,
            Vec2::RIGHT,
            true,
            false,
            false,
            false,
            false,
        );
        session.peer_id_lookup.insert(remote_id.clone(), remote);
        session
            .peer_hash_lookup
            .insert(hash_for(remote), remote_id.clone());
        session.remote_players.insert(
            remote_id.clone(),
            RemotePlayer::new("FAR".to_string(), &state, 1),
        );

        session.recompute_area_authorities(10);

        assert_eq!(
            session.area_authorities.get(&near_area),
            Some(&hash_for(root)),
            "nearest member owns the root's area"
        );
        assert_eq!(
            session.area_authorities.get(&far_area),
            Some(&hash_for(remote)),
            "nearest member owns the remote's area"
        );
        assert_eq!(session.owned_area_ids().len(), 1);

        // Ownership checks: assigned area belongs to its authority...
        assert!(session.area_owned_by(far_area, hash_for(remote)));
        assert!(!session.area_owned_by(far_area, hash_for(root)));
        // ...and unassigned areas default to the host.
        let unassigned = NetworkSession::area_id_for_pos(Vec2::new(-100_000.0, -100_000.0));
        assert!(session.area_owned_by(unassigned, hash_for(root)));
        assert!(!session.area_owned_by(unassigned, hash_for(remote)));
    }

    #[test]
    fn area_authority_is_sticky_against_marginal_challengers() {
        let root = peer_from(0);
        let remote = peer_from(1);

        let mut session = NetworkSession::new();
        session.is_host = true;
        session.local_peer_id = Some(root);
        session.local_peer_hash = Some(hash_for(root));
        session.super_root_id = Some(root);
        session.topology_roster = vec![entry_for(root, 0), entry_for(remote, hash_for(root))];

        // Both in the same area, equidistant from its center: the incumbent
        // must keep it regardless of hash ordering.
        session.local_last_pos = Some(Vec2::new(0.0, 0.0));
        let near = Vec2::new(64.0, 0.0);
        let shared_area = NetworkSession::area_id_for_pos(Vec2::new(0.0, 0.0));
        assert_eq!(shared_area, NetworkSession::area_id_for_pos(near));

        let remote_id = format!("{:?}", remote);
        let state = PlayerState::new(
            1,
            near,
            Vec2::RIGHT,
            Vec2::RIGHT,
            true,
            false,
            false,
            false,
            false,
        );
        session.peer_id_lookup.insert(remote_id.clone(), remote);
        session
            .peer_hash_lookup
            .insert(hash_for(remote), remote_id.clone());
        session.remote_players.insert(
            remote_id.clone(),
            RemotePlayer::new("NEAR".to_string(), &state, 1),
        );

        // Seed the remote as incumbent; an (at best) marginally closer root
        // must not steal the area.
        session
            .area_authorities
            .insert(shared_area, hash_for(remote));
        session.recompute_area_authorities(10);
        assert_eq!(
            session.area_authorities.get(&shared_area),
            Some(&hash_for(remote)),
            "incumbent keeps the area on marginal differences"
        );
    }

    #[test]
    fn throttled_root_hands_off_and_recipient_seeds_liveness() {
        let root = peer_from(0);
        let a = peer_from(1);
        let b = peer_from(2);

        let mut session = NetworkSession::new();
        session.is_host = true;
        session.local_peer_id = Some(root);
        session.local_peer_hash = Some(hash_for(root));
        session.relay_fanout = 4;
        session.relay_epoch = 7;
        session.topology_roster = vec![
            entry_for(root, 0),
            entry_for(a, hash_for(root)),
            entry_for(b, hash_for(root)),
        ];

        session.handoff_root(100);
        assert!(!session.is_host, "old root must drop the role");
        // Successor is the lowest-uuid member.
        assert_eq!(session.roster_root_hash(), Some(hash_for(a)));
        assert_eq!(session.relay_parent, Some(a));
        assert!(session.relay_epoch > 7);

        // The successor adopts the map and must seed liveness for inherited
        // members so its first root pass doesn't prune everyone.
        let map = TopologyUpdate {
            epoch: session.relay_epoch,
            root_hash: hash_for(a),
            fanout: 4,
            entries: session.topology_roster.clone(),
        };
        let mut successor = NetworkSession::new();
        successor.local_peer_id = Some(a);
        successor.local_peer_hash = Some(hash_for(a));
        assert!(successor.apply_topology_map("sender", &map, 200));
        assert!(
            successor.is_host,
            "successor must take the role from the map"
        );
        successor.update_topology_as_root(210, &[]);
        assert!(successor.roster_contains(hash_for(b)));
        assert!(successor.roster_contains(hash_for(root)));
    }

    #[test]
    fn fanout_override_builds_chains() {
        let root = peer_from(0);
        let mut session = NetworkSession::new();
        session.is_host = true;
        session.local_peer_id = Some(root);
        session.local_peer_hash = Some(hash_for(root));
        session.set_fanout_override(1);
        session.topology_roster.push(entry_for(root, 0));
        session.update_topology_as_root(10, &[]);
        for i in 1..=3u128 {
            session.add_roster_member(NetworkSession::uuid_for_peer(peer_from(i)), 10);
        }
        // Chain: each member has exactly one child.
        for entry in &session.topology_roster {
            assert!(
                session.roster_child_count(entry.peer_hash) <= 1,
                "fanout 1 must build a chain"
            );
        }
    }

    #[test]
    fn root_departure_evidence_triggers_fast_failover() {
        let root = peer_from(0);
        let first = peer_from(1);
        let second = peer_from(2);

        let mut session = NetworkSession::new();
        session.local_peer_id = Some(first);
        session.local_peer_hash = Some(hash_for(first));
        session.topology_roster = vec![
            entry_for(root, 0),
            entry_for(first, hash_for(root)),
            entry_for(second, hash_for(root)),
        ];
        session.relay_epoch = 5;
        session.last_map_rx_frame = 100;
        // Map is fresh; without evidence nothing happens.
        session.maybe_root_failover(200, &[]);
        assert!(!session.is_host);

        // Departure evidence arms the fast path even with a fresh map.
        session.root_departure_frame = Some(200);
        let rank = {
            let mut uuids = vec![
                NetworkSession::uuid_for_peer(first),
                NetworkSession::uuid_for_peer(second),
            ];
            uuids.sort();
            uuids
                .iter()
                .position(|u| *u == NetworkSession::uuid_for_peer(first))
                .unwrap() as u32
        };
        let due = 200
            + NetworkSession::ROOT_DEPARTURE_TAKEOVER_BASE_FRAMES
            + rank * NetworkSession::ROOT_DEPARTURE_TAKEOVER_STAGGER_FRAMES;
        session.maybe_root_failover(due - 1, &[]);
        assert!(!session.is_host, "promoted before fast window elapsed");
        session.maybe_root_failover(due, &[]);
        assert!(session.is_host, "fast failover did not promote");
        assert_eq!(session.roster_root_hash(), Some(hash_for(first)));
        // The other member survives the takeover with a fresh slot.
        assert!(session.roster_contains(hash_for(second)));
    }

    #[test]
    fn host_defers_only_to_outranking_root() {
        let me = peer_from(5);
        let other_low = peer_from(1);

        let mut session = NetworkSession::new();
        session.is_host = true;
        session.local_peer_id = Some(me);
        session.update_topology_as_root(10, &[]);
        let my_epoch = session.relay_epoch;

        // Same epoch, higher uuid-hash root: we keep the room.
        let weak = TopologyUpdate {
            epoch: my_epoch,
            root_hash: u64::MAX,
            fanout: 2,
            entries: vec![TopologyEntry {
                peer_hash: u64::MAX,
                uuid: [9u8; 16],
                parent_hash: 0,
            }],
        };
        assert!(!session.apply_topology_map("sender", &weak, 20));
        assert!(session.is_host);

        // Strictly newer epoch: we abdicate and adopt.
        let strong = TopologyUpdate {
            epoch: my_epoch + 10,
            root_hash: hash_for(other_low),
            fanout: 2,
            entries: vec![entry_for(other_low, 0), entry_for(me, hash_for(other_low))],
        };
        assert!(session.apply_topology_map("sender", &strong, 30));
        assert!(!session.is_host);
        assert_eq!(session.super_root_id, Some(other_low));
        assert_eq!(session.relay_parent, Some(other_low));
    }

    #[test]
    fn desired_links_are_stable_across_epoch_bumps() {
        let peers = known_peers(12);
        let local = peers[0];

        let mut session = NetworkSession::new();
        session.local_peer_id = Some(local);
        session.is_host = false;
        session.relay_parent = Some(peers[1]);
        session.relay_backup_parent = Some(peers[2]);
        session.relay_children = vec![peers[3], peers[4]];

        session.relay_epoch = 7;
        let first = session.desired_peer_links(&peers, &peers);
        session.relay_epoch = 8;
        let second = session.desired_peer_links(&peers, &peers);
        assert_eq!(first, second, "epoch bumps must not rotate links");
    }

    #[test]
    fn undesired_links_get_grace_before_drop() {
        let peers = known_peers(3);
        let local = peers[0];
        let stray = peers[2];

        let mut session = NetworkSession::new();
        session.local_peer_id = Some(local);
        session.is_host = true;
        session.relay_children = vec![peers[1]];
        // All three are roster members, so the stray is neither a tree link
        // nor an admission candidate for us.
        session.topology_roster = vec![
            entry_for(local, 0),
            entry_for(peers[1], hash_for(local)),
            entry_for(stray, hash_for(peers[1])),
        ];

        session.last_update_frame = 100;
        session.update_desired_peers(&peers, &[peers[1], stray]);
        // The stray link is tracked but kept during the grace window.
        assert!(session.undesired_since.contains_key(&stray));

        // Becoming desired again clears the timer.
        session.relay_children = vec![peers[1], stray];
        session.last_update_frame = 200;
        session.update_desired_peers(&peers, &[peers[1], stray]);
        assert!(!session.undesired_since.contains_key(&stray));
    }

    #[test]
    fn desired_links_exclude_temporarily_backed_off_peers() {
        let peers = known_peers(4);
        let local = peers[0];
        let silent = peers[1];
        let healthy = peers[2];

        let mut session = NetworkSession::new();
        session.local_peer_id = Some(local);
        session.is_host = true;
        session.discovery_attached = true;
        session.relay_children = vec![silent, healthy];
        session.last_update_frame = 100;
        session
            .peer_link_backoff_until
            .insert(silent, session.last_update_frame + 30);

        let desired = session.desired_peer_links(&peers, &peers[1..3]);

        assert!(!desired.contains(&silent));
        assert!(desired.contains(&healthy));
    }
}

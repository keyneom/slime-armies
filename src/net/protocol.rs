use crate::math::Vec2;

// ============ Enemy Types for Network Sync ============

/// Enemy type identifier (1 byte)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum EnemyType {
    Spider = 0,
    Cannon = 1,
    Snake = 2,
    Wisp = 3,
    Guardian = 4,
}

impl EnemyType {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Spider),
            1 => Some(Self::Cannon),
            2 => Some(Self::Snake),
            3 => Some(Self::Wisp),
            4 => Some(Self::Guardian),
            _ => None,
        }
    }
}

/// Compact enemy state for network transmission (21 bytes)
/// Used for all enemy types - some fields may be unused for certain types
#[derive(Debug, Clone, Copy)]
pub struct EnemyState {
    pub id: u16,        // Enemy ID (2 bytes)
    pub enemy_type: u8, // EnemyType (1 byte)
    pub flags: u8,      // alive flag + other bits (1 byte)
    pub x: f32,         // Position X (4 bytes)
    pub y: f32,         // Position Y (4 bytes)
    pub dir_x: f32,     // Direction/look_dir X (4 bytes)
    pub dir_y: f32,     // Direction/look_dir Y (4 bytes)
    pub extra: u8,      // Snake size / cannon shoot timer low bits (1 byte)
}

impl EnemyState {
    pub fn new_spider(id: usize, alive: bool, pos: Vec2, dir: Vec2) -> Self {
        Self {
            id: id as u16,
            enemy_type: EnemyType::Spider as u8,
            flags: alive as u8,
            x: pos.x,
            y: pos.y,
            dir_x: dir.x,
            dir_y: dir.y,
            extra: 0,
        }
    }

    pub fn new_cannon(id: usize, alive: bool, pos: Vec2, look_dir: Vec2) -> Self {
        Self {
            id: id as u16,
            enemy_type: EnemyType::Cannon as u8,
            flags: alive as u8,
            x: pos.x,
            y: pos.y,
            dir_x: look_dir.x,
            dir_y: look_dir.y,
            extra: 0,
        }
    }

    pub fn new_snake(id: usize, alive: bool, pos: Vec2, dir: Vec2, size: f32) -> Self {
        Self {
            id: id as u16,
            enemy_type: EnemyType::Snake as u8,
            flags: alive as u8,
            x: pos.x,
            y: pos.y,
            dir_x: dir.x,
            dir_y: dir.y,
            extra: (size as u8).min(255),
        }
    }

    pub fn new_wisp(id: usize, alive: bool, pos: Vec2, dir: Vec2) -> Self {
        Self {
            id: id as u16,
            enemy_type: EnemyType::Wisp as u8,
            flags: alive as u8,
            x: pos.x,
            y: pos.y,
            dir_x: dir.x,
            dir_y: dir.y,
            extra: 0,
        }
    }

    pub fn new_guardian(id: usize, alive: bool, pos: Vec2, dir: Vec2) -> Self {
        Self {
            id: id as u16,
            enemy_type: EnemyType::Guardian as u8,
            flags: alive as u8,
            x: pos.x,
            y: pos.y,
            dir_x: dir.x,
            dir_y: dir.y,
            extra: 0,
        }
    }

    pub fn pos(&self) -> Vec2 {
        Vec2::new(self.x, self.y)
    }

    pub fn dir(&self) -> Vec2 {
        Vec2::new(self.dir_x, self.dir_y)
    }

    pub fn is_alive(&self) -> bool {
        (self.flags & 1) != 0
    }

    pub fn get_type(&self) -> Option<EnemyType> {
        EnemyType::from_u8(self.enemy_type)
    }

    pub fn snake_size(&self) -> f32 {
        self.extra as f32
    }

    pub fn to_bytes(&self) -> [u8; 21] {
        let mut bytes = [0u8; 21];
        bytes[0..2].copy_from_slice(&self.id.to_le_bytes());
        bytes[2] = self.enemy_type;
        bytes[3] = self.flags;
        bytes[4..8].copy_from_slice(&self.x.to_le_bytes());
        bytes[8..12].copy_from_slice(&self.y.to_le_bytes());
        bytes[12..16].copy_from_slice(&self.dir_x.to_le_bytes());
        bytes[16..20].copy_from_slice(&self.dir_y.to_le_bytes());
        bytes[20] = self.extra;
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 21 {
            return None;
        }
        Some(Self {
            id: u16::from_le_bytes([bytes[0], bytes[1]]),
            enemy_type: bytes[2],
            flags: bytes[3],
            x: f32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
            y: f32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
            dir_x: f32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]),
            dir_y: f32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]),
            extra: bytes[20],
        })
    }
}

/// Full enemy sync message - sent periodically by host
#[derive(Debug, Clone)]
pub struct EnemySync {
    pub tick: u32,
    pub wave: u32,
    pub enemies: Vec<EnemyState>,
}

impl EnemySync {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&self.tick.to_le_bytes());
        bytes.extend_from_slice(&self.wave.to_le_bytes());
        bytes.extend_from_slice(&(self.enemies.len() as u16).to_le_bytes());
        for enemy in &self.enemies {
            bytes.extend_from_slice(&enemy.to_bytes());
        }
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 10 {
            return None;
        }
        let tick = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let wave = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        let count = u16::from_le_bytes([bytes[8], bytes[9]]) as usize;

        let mut enemies = Vec::with_capacity(count);
        let mut offset = 10;
        for _ in 0..count {
            if offset + 21 > bytes.len() {
                break;
            }
            if let Some(enemy) = EnemyState::from_bytes(&bytes[offset..]) {
                enemies.push(enemy);
            }
            offset += 21;
        }

        Some(Self {
            tick,
            wave,
            enemies,
        })
    }
}

/// Enemy damage event - sent by client when they damage an enemy
#[derive(Debug, Clone, Copy)]
pub struct EnemyDamage {
    pub enemy_type: u8,
    pub enemy_id: u16,
    pub killed: bool,
}

impl EnemyDamage {
    pub fn to_bytes(&self) -> [u8; 4] {
        let mut bytes = [0u8; 4];
        bytes[0] = self.enemy_type;
        bytes[1..3].copy_from_slice(&self.enemy_id.to_le_bytes());
        bytes[3] = self.killed as u8;
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 4 {
            return None;
        }
        Some(Self {
            enemy_type: bytes[0],
            enemy_id: u16::from_le_bytes([bytes[1], bytes[2]]),
            killed: bytes[3] != 0,
        })
    }
}

// ============ Cannon Shots ============

#[derive(Debug, Clone, Copy)]
pub struct CannonShot {
    pub id: u32,
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
}

impl CannonShot {
    pub fn to_bytes(&self) -> [u8; 20] {
        let mut bytes = [0u8; 20];
        bytes[0..4].copy_from_slice(&self.id.to_le_bytes());
        bytes[4..8].copy_from_slice(&self.x.to_le_bytes());
        bytes[8..12].copy_from_slice(&self.y.to_le_bytes());
        bytes[12..16].copy_from_slice(&self.vx.to_le_bytes());
        bytes[16..20].copy_from_slice(&self.vy.to_le_bytes());
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 20 {
            return None;
        }
        Some(Self {
            id: u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            x: f32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
            y: f32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
            vx: f32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]),
            vy: f32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]),
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ProjectileReflection {
    pub id: u32,
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
}

impl ProjectileReflection {
    pub fn to_bytes(&self) -> [u8; 20] {
        let mut bytes = [0u8; 20];
        bytes[0..4].copy_from_slice(&self.id.to_le_bytes());
        bytes[4..8].copy_from_slice(&self.x.to_le_bytes());
        bytes[8..12].copy_from_slice(&self.y.to_le_bytes());
        bytes[12..16].copy_from_slice(&self.vx.to_le_bytes());
        bytes[16..20].copy_from_slice(&self.vy.to_le_bytes());
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 20 {
            return None;
        }
        Some(Self {
            id: u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            x: f32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
            y: f32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
            vx: f32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]),
            vy: f32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]),
        })
    }
}

// ============ Rollback Netcode (Scaffold) ============

#[derive(Debug, Clone, Copy)]
pub struct InputFrame {
    pub frame: u32,
    pub input: u16,
}

impl InputFrame {
    pub fn to_bytes(&self) -> [u8; 6] {
        let mut bytes = [0u8; 6];
        bytes[0..4].copy_from_slice(&self.frame.to_le_bytes());
        bytes[4..6].copy_from_slice(&self.input.to_le_bytes());
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 6 {
            return None;
        }
        Some(Self {
            frame: u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            input: u16::from_le_bytes([bytes[4], bytes[5]]),
        })
    }
}

// ============ Supernode Selection ============

#[derive(Debug, Clone, Copy)]
pub struct Ping {
    pub nonce: u32,
    pub sent_ms: u32,
}

impl Ping {
    pub fn to_bytes(&self) -> [u8; 8] {
        let mut bytes = [0u8; 8];
        bytes[0..4].copy_from_slice(&self.nonce.to_le_bytes());
        bytes[4..8].copy_from_slice(&self.sent_ms.to_le_bytes());
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 8 {
            return None;
        }
        Some(Self {
            nonce: u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            sent_ms: u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Pong {
    pub nonce: u32,
    pub sent_ms: u32,
}

impl Pong {
    pub fn to_bytes(&self) -> [u8; 8] {
        let mut bytes = [0u8; 8];
        bytes[0..4].copy_from_slice(&self.nonce.to_le_bytes());
        bytes[4..8].copy_from_slice(&self.sent_ms.to_le_bytes());
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 8 {
            return None;
        }
        Some(Self {
            nonce: u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            sent_ms: u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct SupernodeScore {
    pub score_ms: u32,
    pub sample_count: u8,
}

impl SupernodeScore {
    pub fn to_bytes(&self) -> [u8; 5] {
        let mut bytes = [0u8; 5];
        bytes[0..4].copy_from_slice(&self.score_ms.to_le_bytes());
        bytes[4] = self.sample_count;
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 5 {
            return None;
        }
        Some(Self {
            score_ms: u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            sample_count: bytes[4],
        })
    }
}

// ============ Paid Obstacles ============

/// Paid obstacle placement proof (fixed 32-byte hash)
#[derive(Debug, Clone, Copy)]
pub struct PaidObstacle {
    pub x: f32,
    pub y: f32,
    pub radius: f32,
    pub variant: u8,
    pub proof_hash: [u8; 32],
}

impl PaidObstacle {
    pub fn to_bytes(&self) -> [u8; 45] {
        let mut bytes = [0u8; 45];
        bytes[0..4].copy_from_slice(&self.x.to_le_bytes());
        bytes[4..8].copy_from_slice(&self.y.to_le_bytes());
        bytes[8..12].copy_from_slice(&self.radius.to_le_bytes());
        bytes[12] = self.variant;
        bytes[13..45].copy_from_slice(&self.proof_hash);
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 45 {
            return None;
        }
        let mut proof_hash = [0u8; 32];
        proof_hash.copy_from_slice(&bytes[13..45]);
        Some(Self {
            x: f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            y: f32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
            radius: f32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
            variant: bytes[12],
            proof_hash,
        })
    }
}

// ============ Paid Abilities ============

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PaidAbilityType {
    BubbleShield = 0,
    Shockwave = 1,
    SlowSpawn = 2,
    SpeedBoost = 3,
    SlimeTrail = 4,
}

impl PaidAbilityType {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::BubbleShield),
            1 => Some(Self::Shockwave),
            2 => Some(Self::SlowSpawn),
            3 => Some(Self::SpeedBoost),
            4 => Some(Self::SlimeTrail),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PaidAbility {
    pub ability_type: u8,
    pub x: f32,
    pub y: f32,
    pub radius: f32,
    pub nonce: u32,
    pub proof_hash: [u8; 32],
}

impl PaidAbility {
    pub fn to_bytes(&self) -> [u8; 49] {
        let mut bytes = [0u8; 49];
        bytes[0] = self.ability_type;
        bytes[1..5].copy_from_slice(&self.x.to_le_bytes());
        bytes[5..9].copy_from_slice(&self.y.to_le_bytes());
        bytes[9..13].copy_from_slice(&self.radius.to_le_bytes());
        bytes[13..17].copy_from_slice(&self.nonce.to_le_bytes());
        bytes[17..49].copy_from_slice(&self.proof_hash);
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 49 {
            return None;
        }
        let mut proof_hash = [0u8; 32];
        proof_hash.copy_from_slice(&bytes[17..49]);
        Some(Self {
            ability_type: bytes[0],
            x: f32::from_le_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]),
            y: f32::from_le_bytes([bytes[5], bytes[6], bytes[7], bytes[8]]),
            radius: f32::from_le_bytes([bytes[9], bytes[10], bytes[11], bytes[12]]),
            nonce: u32::from_le_bytes([bytes[13], bytes[14], bytes[15], bytes[16]]),
            proof_hash,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PaidAbilityAck {
    pub proof_hash: [u8; 32],
}

impl PaidAbilityAck {
    pub fn to_bytes(&self) -> [u8; 32] {
        self.proof_hash
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 32 {
            return None;
        }
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&bytes[0..32]);
        Some(Self { proof_hash: hash })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PaidNameReservation {
    pub owner_hash: u64,
    pub nonce: u32,
    pub name: [u8; 20],
    pub proof_hash: [u8; 32],
}

impl PaidNameReservation {
    pub fn from_name(owner_hash: u64, name: &str, nonce: u32, proof_hash: [u8; 32]) -> Self {
        let mut name_bytes = [0u8; 20];
        let normalized: String = name
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(20)
            .map(|c| c.to_ascii_uppercase())
            .collect();
        let bytes = normalized.as_bytes();
        let len = bytes.len().min(20);
        name_bytes[..len].copy_from_slice(&bytes[..len]);
        Self {
            owner_hash,
            nonce,
            name: name_bytes,
            proof_hash,
        }
    }

    pub fn name_string(&self) -> String {
        let end = self
            .name
            .iter()
            .position(|b| *b == 0)
            .unwrap_or(self.name.len());
        String::from_utf8_lossy(&self.name[..end]).to_string()
    }

    pub fn to_bytes(&self) -> [u8; 64] {
        let mut bytes = [0u8; 64];
        bytes[0..8].copy_from_slice(&self.owner_hash.to_le_bytes());
        bytes[8..12].copy_from_slice(&self.nonce.to_le_bytes());
        bytes[12..32].copy_from_slice(&self.name);
        bytes[32..64].copy_from_slice(&self.proof_hash);
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 64 {
            return None;
        }
        let mut name = [0u8; 20];
        let mut proof_hash = [0u8; 32];
        name.copy_from_slice(&bytes[12..32]);
        proof_hash.copy_from_slice(&bytes[32..64]);
        Some(Self {
            owner_hash: u64::from_le_bytes([
                bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            ]),
            nonce: u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
            name,
            proof_hash,
        })
    }
}

#[derive(Debug, Clone)]
pub struct PaidNameSync {
    pub reservations: Vec<PaidNameReservation>,
}

impl PaidNameSync {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(self.reservations.len() as u16).to_le_bytes());
        for reservation in &self.reservations {
            bytes.extend_from_slice(&reservation.to_bytes());
        }
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 2 {
            return None;
        }
        let count = u16::from_le_bytes([bytes[0], bytes[1]]) as usize;
        let mut reservations = Vec::with_capacity(count);
        let mut offset = 2;
        for _ in 0..count {
            if offset + 64 > bytes.len() {
                break;
            }
            if let Some(reservation) = PaidNameReservation::from_bytes(&bytes[offset..offset + 64])
            {
                reservations.push(reservation);
            }
            offset += 64;
        }
        Some(Self { reservations })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PaidNameAck {
    pub proof_hash: [u8; 32],
}

impl PaidNameAck {
    pub fn to_bytes(&self) -> [u8; 32] {
        self.proof_hash
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 32 {
            return None;
        }
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&bytes[0..32]);
        Some(Self { proof_hash: hash })
    }
}

#[derive(Debug, Clone)]
pub struct PaidObstacleSync {
    pub obstacles: Vec<PaidObstacle>,
}

impl PaidObstacleSync {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(self.obstacles.len() as u16).to_le_bytes());
        for obstacle in &self.obstacles {
            bytes.extend_from_slice(&obstacle.to_bytes());
        }
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 2 {
            return None;
        }
        let count = u16::from_le_bytes([bytes[0], bytes[1]]) as usize;
        let mut obstacles = Vec::with_capacity(count);
        let mut offset = 2;
        for _ in 0..count {
            if offset + 45 > bytes.len() {
                break;
            }
            if let Some(obstacle) = PaidObstacle::from_bytes(&bytes[offset..offset + 45]) {
                obstacles.push(obstacle);
            }
            offset += 45;
        }
        Some(Self { obstacles })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PaidObstacleAck {
    pub proof_hash: [u8; 32],
}

impl PaidObstacleAck {
    pub fn to_bytes(&self) -> [u8; 32] {
        self.proof_hash
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 32 {
            return None;
        }
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&bytes[0..32]);
        Some(Self { proof_hash: hash })
    }
}

// ============ Deterministic Sync Messages ============

/// Wave start event - shares RNG seed so all clients spawn identical enemies
#[derive(Debug, Clone, Copy)]
pub struct WaveStart {
    pub wave: u32,
    pub rng_seed: u64,
    pub spider_count: u16,
    pub cannon_count: u16,
    pub snake_count: u16,
    pub wisp_count: u16,
    pub spawn_x: f32,
    pub spawn_y: f32,
}

impl WaveStart {
    pub fn to_bytes(&self) -> [u8; 28] {
        let mut bytes = [0u8; 28];
        bytes[0..4].copy_from_slice(&self.wave.to_le_bytes());
        bytes[4..12].copy_from_slice(&self.rng_seed.to_le_bytes());
        bytes[12..14].copy_from_slice(&self.spider_count.to_le_bytes());
        bytes[14..16].copy_from_slice(&self.cannon_count.to_le_bytes());
        bytes[16..18].copy_from_slice(&self.snake_count.to_le_bytes());
        bytes[18..20].copy_from_slice(&self.wisp_count.to_le_bytes());
        bytes[20..24].copy_from_slice(&self.spawn_x.to_le_bytes());
        bytes[24..28].copy_from_slice(&self.spawn_y.to_le_bytes());
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 28 {
            return None;
        }
        Some(Self {
            wave: u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            rng_seed: u64::from_le_bytes([
                bytes[4], bytes[5], bytes[6], bytes[7], bytes[8], bytes[9], bytes[10], bytes[11],
            ]),
            spider_count: u16::from_le_bytes([bytes[12], bytes[13]]),
            cannon_count: u16::from_le_bytes([bytes[14], bytes[15]]),
            snake_count: u16::from_le_bytes([bytes[16], bytes[17]]),
            wisp_count: u16::from_le_bytes([bytes[18], bytes[19]]),
            spawn_x: f32::from_le_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]),
            spawn_y: f32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]),
        })
    }
}

/// Enemy kill event - authoritative from the killer's machine
#[derive(Debug, Clone, Copy)]
pub struct EnemyKill {
    pub enemy_type: u8,
    pub enemy_id: u16,
    pub killer_x: f32, // Position where kill happened (for verification)
    pub killer_y: f32,
    pub killer_hash: u64,
    pub event_id: u64,
}

/// Batched enemy kill events to reduce per-message overhead
#[derive(Debug, Clone)]
pub struct EnemyKillBatch {
    pub kills: Vec<EnemyKill>,
}

impl EnemyKillBatch {
    pub fn to_bytes(&self) -> Vec<u8> {
        let count = self.kills.len().min(u16::MAX as usize);
        let mut bytes = Vec::with_capacity(2 + count * 27);
        bytes.extend_from_slice(&(count as u16).to_le_bytes());
        for kill in self.kills.iter().take(count) {
            bytes.extend_from_slice(&kill.to_bytes());
        }
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 2 {
            return None;
        }
        let count = u16::from_le_bytes([bytes[0], bytes[1]]) as usize;
        let mut kills = Vec::with_capacity(count);
        let mut offset = 2;
        for _ in 0..count {
            if offset + 27 > bytes.len() {
                break;
            }
            if let Some(kill) = EnemyKill::from_bytes(&bytes[offset..offset + 27]) {
                kills.push(kill);
            }
            offset += 27;
        }
        Some(Self { kills })
    }
}

impl EnemyKill {
    pub fn to_bytes(&self) -> [u8; 27] {
        let mut bytes = [0u8; 27];
        bytes[0] = self.enemy_type;
        bytes[1..3].copy_from_slice(&self.enemy_id.to_le_bytes());
        bytes[3..7].copy_from_slice(&self.killer_x.to_le_bytes());
        bytes[7..11].copy_from_slice(&self.killer_y.to_le_bytes());
        bytes[11..19].copy_from_slice(&self.killer_hash.to_le_bytes());
        bytes[19..27].copy_from_slice(&self.event_id.to_le_bytes());
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 27 {
            return None;
        }
        Some(Self {
            enemy_type: bytes[0],
            enemy_id: u16::from_le_bytes([bytes[1], bytes[2]]),
            killer_x: f32::from_le_bytes([bytes[3], bytes[4], bytes[5], bytes[6]]),
            killer_y: f32::from_le_bytes([bytes[7], bytes[8], bytes[9], bytes[10]]),
            killer_hash: u64::from_le_bytes([
                bytes[11], bytes[12], bytes[13], bytes[14], bytes[15], bytes[16], bytes[17],
                bytes[18],
            ]),
            event_id: u64::from_le_bytes([
                bytes[19], bytes[20], bytes[21], bytes[22], bytes[23], bytes[24], bytes[25],
                bytes[26],
            ]),
        })
    }
}

/// Player death event - authoritative from the victim's machine
#[derive(Debug, Clone, Copy)]
pub struct PlayerDeath {
    pub death_x: f32,
    pub death_y: f32,
    pub killed_by_type: u8, // 0=spider, 1=cannon, 2=snake, 3=projectile, 4=wisp
    pub killed_by_id: u16,
    pub victim_hash: u64,
    pub event_id: u64,
}

/// Chat message (room-wide)
#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub sender_hash: u64,
    pub text: String,
}

impl ChatMessage {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&self.sender_hash.to_le_bytes());
        let len = self.text.len().min(80) as u8;
        bytes.push(len);
        bytes.extend_from_slice(&self.text.as_bytes()[..len as usize]);
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 9 {
            return None;
        }
        let hash = u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]);
        let len = bytes[8] as usize;
        if bytes.len() < 9 + len {
            return None;
        }
        let text = String::from_utf8(bytes[9..9 + len].to_vec()).ok()?;
        Some(Self {
            sender_hash: hash,
            text,
        })
    }
}

/// Vote mute request
#[derive(Debug, Clone, Copy)]
pub struct VoteMute {
    pub target_hash: u64,
    pub voter_hash: u64,
}

impl VoteMute {
    pub fn to_bytes(&self) -> [u8; 16] {
        let mut bytes = [0u8; 16];
        bytes[0..8].copy_from_slice(&self.target_hash.to_le_bytes());
        bytes[8..16].copy_from_slice(&self.voter_hash.to_le_bytes());
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 16 {
            return None;
        }
        Some(Self {
            target_hash: u64::from_le_bytes([
                bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            ]),
            voter_hash: u64::from_le_bytes([
                bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14],
                bytes[15],
            ]),
        })
    }
}
impl PlayerDeath {
    pub fn to_bytes(&self) -> [u8; 27] {
        let mut bytes = [0u8; 27];
        bytes[0..4].copy_from_slice(&self.death_x.to_le_bytes());
        bytes[4..8].copy_from_slice(&self.death_y.to_le_bytes());
        bytes[8] = self.killed_by_type;
        bytes[9..11].copy_from_slice(&self.killed_by_id.to_le_bytes());
        bytes[11..19].copy_from_slice(&self.victim_hash.to_le_bytes());
        bytes[19..27].copy_from_slice(&self.event_id.to_le_bytes());
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 27 {
            return None;
        }
        Some(Self {
            death_x: f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            death_y: f32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
            killed_by_type: bytes[8],
            killed_by_id: u16::from_le_bytes([bytes[9], bytes[10]]),
            victim_hash: u64::from_le_bytes([
                bytes[11], bytes[12], bytes[13], bytes[14], bytes[15], bytes[16], bytes[17],
                bytes[18],
            ]),
            event_id: u64::from_le_bytes([
                bytes[19], bytes[20], bytes[21], bytes[22], bytes[23], bytes[24], bytes[25],
                bytes[26],
            ]),
        })
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PlayerStatsSnapshot {
    pub player_hash: u64,
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

impl PlayerStatsSnapshot {
    pub fn to_bytes(&self) -> [u8; 44] {
        let mut bytes = [0u8; 44];
        bytes[0..8].copy_from_slice(&self.player_hash.to_le_bytes());
        bytes[8..12].copy_from_slice(&self.kills.to_le_bytes());
        bytes[12..16].copy_from_slice(&self.spider_kills.to_le_bytes());
        bytes[16..20].copy_from_slice(&self.cannon_kills.to_le_bytes());
        bytes[20..24].copy_from_slice(&self.snake_kills.to_le_bytes());
        bytes[24..28].copy_from_slice(&self.wisp_kills.to_le_bytes());
        bytes[28..32].copy_from_slice(&self.attack_attempts.to_le_bytes());
        bytes[32..36].copy_from_slice(&self.attack_hits.to_le_bytes());
        bytes[36..40].copy_from_slice(&self.deaths.to_le_bytes());
        bytes[40..44].copy_from_slice(&self.time_played_frames.to_le_bytes());
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 44 {
            return None;
        }
        Some(Self {
            player_hash: u64::from_le_bytes([
                bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
            ]),
            kills: u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
            spider_kills: u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]),
            cannon_kills: u32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]),
            snake_kills: u32::from_le_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]),
            wisp_kills: u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]),
            attack_attempts: u32::from_le_bytes([bytes[28], bytes[29], bytes[30], bytes[31]]),
            attack_hits: u32::from_le_bytes([bytes[32], bytes[33], bytes[34], bytes[35]]),
            deaths: u32::from_le_bytes([bytes[36], bytes[37], bytes[38], bytes[39]]),
            time_played_frames: u32::from_le_bytes([bytes[40], bytes[41], bytes[42], bytes[43]]),
        })
    }
}

// ============ Player State ============

/// Compact player state for network transmission (29 bytes)
#[derive(Debug, Clone, Copy)]
pub struct PlayerState {
    pub sim_frame: u32,
    pub x: f32,
    pub y: f32,
    pub look_dir_x: f32,
    pub look_dir_y: f32,
    pub move_dir_x: f32, // For tail animation
    pub move_dir_y: f32,
    pub flags: u8, // alive|attacking|blocking|phasing|shielded|_|_|_
}

impl PlayerState {
    pub fn new(
        sim_frame: u32,
        pos: Vec2,
        look_dir: Vec2,
        move_dir: Vec2,
        alive: bool,
        attacking: bool,
        blocking: bool,
        phasing: bool,
        shielded: bool,
    ) -> Self {
        let flags = (alive as u8)
            | ((attacking as u8) << 1)
            | ((blocking as u8) << 2)
            | ((phasing as u8) << 3)
            | ((shielded as u8) << 4);

        Self {
            sim_frame,
            x: pos.x,
            y: pos.y,
            look_dir_x: look_dir.x,
            look_dir_y: look_dir.y,
            move_dir_x: move_dir.x,
            move_dir_y: move_dir.y,
            flags,
        }
    }

    pub fn sim_frame(&self) -> u32 {
        self.sim_frame
    }

    pub fn pos(&self) -> Vec2 {
        Vec2::new(self.x, self.y)
    }

    pub fn look_dir(&self) -> Vec2 {
        Vec2::new(self.look_dir_x, self.look_dir_y)
    }

    pub fn move_dir(&self) -> Vec2 {
        Vec2::new(self.move_dir_x, self.move_dir_y)
    }

    pub fn is_alive(&self) -> bool {
        (self.flags & 1) != 0
    }

    pub fn is_attacking(&self) -> bool {
        (self.flags & 2) != 0
    }

    pub fn is_blocking(&self) -> bool {
        (self.flags & 4) != 0
    }

    pub fn is_phasing(&self) -> bool {
        (self.flags & 8) != 0
    }

    pub fn is_shielded(&self) -> bool {
        (self.flags & 16) != 0
    }

    pub fn to_bytes(&self) -> [u8; 29] {
        let mut bytes = [0u8; 29];
        bytes[0..4].copy_from_slice(&self.sim_frame.to_le_bytes());
        bytes[4..8].copy_from_slice(&self.x.to_le_bytes());
        bytes[8..12].copy_from_slice(&self.y.to_le_bytes());
        bytes[12..16].copy_from_slice(&self.look_dir_x.to_le_bytes());
        bytes[16..20].copy_from_slice(&self.look_dir_y.to_le_bytes());
        bytes[20..24].copy_from_slice(&self.move_dir_x.to_le_bytes());
        bytes[24..28].copy_from_slice(&self.move_dir_y.to_le_bytes());
        bytes[28] = self.flags;
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 29 {
            return None;
        }
        Some(Self {
            sim_frame: u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            x: f32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
            y: f32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
            look_dir_x: f32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]),
            look_dir_y: f32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]),
            move_dir_x: f32::from_le_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]),
            move_dir_y: f32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]),
            flags: bytes[28],
        })
    }
}

// ============ Batched Relay Messages ============

#[derive(Debug, Clone, Copy)]
pub struct PlayerStateEntry {
    pub peer_hash: u64,
    pub area_id: u32,
    pub state: PlayerState,
}

#[derive(Debug, Clone)]
pub struct PlayerStateBatch {
    pub entries: Vec<PlayerStateEntry>,
}

impl PlayerStateBatch {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(self.entries.len() as u16).to_le_bytes());
        for entry in &self.entries {
            bytes.extend_from_slice(&entry.peer_hash.to_le_bytes());
            bytes.extend_from_slice(&entry.area_id.to_le_bytes());
            bytes.extend_from_slice(&entry.state.to_bytes());
        }
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 2 {
            return None;
        }
        let count = u16::from_le_bytes([bytes[0], bytes[1]]) as usize;
        let mut entries = Vec::with_capacity(count);
        let mut offset = 2;
        for _ in 0..count {
            if offset + 8 + 4 + 29 > bytes.len() {
                break;
            }
            let peer_hash = u64::from_le_bytes([
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
                bytes[offset + 4],
                bytes[offset + 5],
                bytes[offset + 6],
                bytes[offset + 7],
            ]);
            offset += 8;
            let area_id = u32::from_le_bytes([
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
            ]);
            offset += 4;
            let state = PlayerState::from_bytes(&bytes[offset..offset + 29])?;
            offset += 29;
            entries.push(PlayerStateEntry {
                peer_hash,
                area_id,
                state,
            });
        }
        Some(Self { entries })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct InputFrameEntry {
    pub peer_hash: u64,
    pub area_id: u32,
    pub frame: InputFrame,
}

#[derive(Debug, Clone)]
pub struct InputFrameBatch {
    pub entries: Vec<InputFrameEntry>,
}

impl InputFrameBatch {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(self.entries.len() as u16).to_le_bytes());
        for entry in &self.entries {
            bytes.extend_from_slice(&entry.peer_hash.to_le_bytes());
            bytes.extend_from_slice(&entry.area_id.to_le_bytes());
            bytes.extend_from_slice(&entry.frame.to_bytes());
        }
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 2 {
            return None;
        }
        let count = u16::from_le_bytes([bytes[0], bytes[1]]) as usize;
        let mut entries = Vec::with_capacity(count);
        let mut offset = 2;
        for _ in 0..count {
            if offset + 8 + 4 + 6 > bytes.len() {
                break;
            }
            let peer_hash = u64::from_le_bytes([
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
                bytes[offset + 4],
                bytes[offset + 5],
                bytes[offset + 6],
                bytes[offset + 7],
            ]);
            offset += 8;
            let area_id = u32::from_le_bytes([
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
            ]);
            offset += 4;
            let frame = InputFrame::from_bytes(&bytes[offset..offset + 6])?;
            offset += 6;
            entries.push(InputFrameEntry {
                peer_hash,
                area_id,
                frame,
            });
        }
        Some(Self { entries })
    }
}

/// One member of the relay tree, as assigned by the root.
/// `uuid` is the raw matchbox PeerId so any node can resolve `peer_hash`
/// to a connectable transport id even for peers it has never seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TopologyEntry {
    pub peer_hash: u64,
    pub uuid: [u8; 16],
    /// 0 = this entry is the root (no parent).
    pub parent_hash: u64,
}

/// Room-wide relay tree map, broadcast by the root and forwarded verbatim
/// down the tree. Identical for every recipient: each node derives its own
/// parent/children/backup from the entries, so forwarding can never assign
/// a node someone else's routes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopologyUpdate {
    pub epoch: u32,
    pub root_hash: u64,
    pub fanout: u8,
    pub entries: Vec<TopologyEntry>,
}

impl TopologyUpdate {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(4 + 8 + 1 + 2 + self.entries.len() * 32);
        bytes.extend_from_slice(&self.epoch.to_le_bytes());
        bytes.extend_from_slice(&self.root_hash.to_le_bytes());
        bytes.push(self.fanout);
        bytes.extend_from_slice(&(self.entries.len() as u16).to_le_bytes());
        for entry in &self.entries {
            bytes.extend_from_slice(&entry.peer_hash.to_le_bytes());
            bytes.extend_from_slice(&entry.uuid);
            bytes.extend_from_slice(&entry.parent_hash.to_le_bytes());
        }
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 4 + 8 + 1 + 2 {
            return None;
        }
        let epoch = u32::from_le_bytes(bytes[0..4].try_into().ok()?);
        let root_hash = u64::from_le_bytes(bytes[4..12].try_into().ok()?);
        let fanout = bytes[12];
        let count = u16::from_le_bytes(bytes[13..15].try_into().ok()?) as usize;
        let mut offset = 15;
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            if offset + 32 > bytes.len() {
                return None;
            }
            let peer_hash = u64::from_le_bytes(bytes[offset..offset + 8].try_into().ok()?);
            let uuid: [u8; 16] = bytes[offset + 8..offset + 24].try_into().ok()?;
            let parent_hash = u64::from_le_bytes(bytes[offset + 24..offset + 32].try_into().ok()?);
            entries.push(TopologyEntry {
                peer_hash,
                uuid,
                parent_hash,
            });
            offset += 32;
        }
        Some(Self {
            epoch,
            root_hash,
            fanout,
            entries,
        })
    }
}

/// Incremental change to the relay-tree map: applies on top of `epoch_from`
/// and yields `epoch_to`. `checksum` is over the resulting full roster so a
/// receiver that diverged detects it immediately and falls back to requesting
/// a full map. At scale this replaces rebroadcasting the whole roster on
/// every membership change (a join is ~40 bytes instead of 32B x members).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopologyDelta {
    pub epoch_from: u32,
    pub epoch_to: u32,
    pub root_hash: u64,
    pub fanout: u8,
    /// FNV over the resulting roster's (peer_hash, parent_hash) pairs in
    /// peer_hash order.
    pub checksum: u64,
    pub removed: Vec<u64>,
    pub upserts: Vec<TopologyEntry>,
}

impl TopologyDelta {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(
            4 + 4 + 8 + 1 + 8 + 2 + self.removed.len() * 8 + 2 + self.upserts.len() * 32,
        );
        bytes.extend_from_slice(&self.epoch_from.to_le_bytes());
        bytes.extend_from_slice(&self.epoch_to.to_le_bytes());
        bytes.extend_from_slice(&self.root_hash.to_le_bytes());
        bytes.push(self.fanout);
        bytes.extend_from_slice(&self.checksum.to_le_bytes());
        bytes.extend_from_slice(&(self.removed.len() as u16).to_le_bytes());
        for hash in &self.removed {
            bytes.extend_from_slice(&hash.to_le_bytes());
        }
        bytes.extend_from_slice(&(self.upserts.len() as u16).to_le_bytes());
        for entry in &self.upserts {
            bytes.extend_from_slice(&entry.peer_hash.to_le_bytes());
            bytes.extend_from_slice(&entry.uuid);
            bytes.extend_from_slice(&entry.parent_hash.to_le_bytes());
        }
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 4 + 4 + 8 + 1 + 8 + 2 {
            return None;
        }
        let epoch_from = u32::from_le_bytes(bytes[0..4].try_into().ok()?);
        let epoch_to = u32::from_le_bytes(bytes[4..8].try_into().ok()?);
        let root_hash = u64::from_le_bytes(bytes[8..16].try_into().ok()?);
        let fanout = bytes[16];
        let checksum = u64::from_le_bytes(bytes[17..25].try_into().ok()?);
        let mut offset = 25;
        let removed_count = u16::from_le_bytes(bytes[offset..offset + 2].try_into().ok()?) as usize;
        offset += 2;
        let mut removed = Vec::with_capacity(removed_count);
        for _ in 0..removed_count {
            if offset + 8 > bytes.len() {
                return None;
            }
            removed.push(u64::from_le_bytes(
                bytes[offset..offset + 8].try_into().ok()?,
            ));
            offset += 8;
        }
        if offset + 2 > bytes.len() {
            return None;
        }
        let upsert_count = u16::from_le_bytes(bytes[offset..offset + 2].try_into().ok()?) as usize;
        offset += 2;
        let mut upserts = Vec::with_capacity(upsert_count);
        for _ in 0..upsert_count {
            if offset + 32 > bytes.len() {
                return None;
            }
            let peer_hash = u64::from_le_bytes(bytes[offset..offset + 8].try_into().ok()?);
            let uuid: [u8; 16] = bytes[offset + 8..offset + 24].try_into().ok()?;
            let parent_hash = u64::from_le_bytes(bytes[offset + 24..offset + 32].try_into().ok()?);
            upserts.push(TopologyEntry {
                peer_hash,
                uuid,
                parent_hash,
            });
            offset += 32;
        }
        Some(Self {
            epoch_from,
            epoch_to,
            root_hash,
            fanout,
            checksum,
            removed,
            upserts,
        })
    }
}

/// Sent by a node that wants (or needs to refresh) a slot in the relay tree.
/// Relayed up the tree until it reaches the root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JoinRequest {
    pub peer_hash: u64,
    pub uuid: [u8; 16],
}

impl JoinRequest {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(24);
        bytes.extend_from_slice(&self.peer_hash.to_le_bytes());
        bytes.extend_from_slice(&self.uuid);
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 24 {
            return None;
        }
        let peer_hash = u64::from_le_bytes(bytes[0..8].try_into().ok()?);
        let uuid: [u8; 16] = bytes[8..24].try_into().ok()?;
        Some(Self { peer_hash, uuid })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct AreaAuthorityEntry {
    pub area_id: u32,
    pub authority_hash: u64,
}

#[derive(Debug, Clone)]
pub struct AreaAuthorityUpdate {
    pub epoch: u32,
    pub entries: Vec<AreaAuthorityEntry>,
}

impl AreaAuthorityUpdate {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&self.epoch.to_le_bytes());
        bytes.extend_from_slice(&(self.entries.len() as u16).to_le_bytes());
        for entry in &self.entries {
            bytes.extend_from_slice(&entry.area_id.to_le_bytes());
            bytes.extend_from_slice(&entry.authority_hash.to_le_bytes());
        }
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 6 {
            return None;
        }
        let epoch = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
        let mut offset = 6;
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            if offset + 12 > bytes.len() {
                return None;
            }
            let area_id = u32::from_le_bytes([
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
            ]);
            let authority_hash = u64::from_le_bytes([
                bytes[offset + 4],
                bytes[offset + 5],
                bytes[offset + 6],
                bytes[offset + 7],
                bytes[offset + 8],
                bytes[offset + 9],
                bytes[offset + 10],
                bytes[offset + 11],
            ]);
            entries.push(AreaAuthorityEntry {
                area_id,
                authority_hash,
            });
            offset += 12;
        }
        Some(Self { epoch, entries })
    }
}

/// Network message types
#[derive(Debug, Clone)]
pub enum NetMessage {
    /// Player state update
    PlayerUpdate(PlayerState),
    /// Player joined the game with their name
    PlayerJoined(String),
    /// Player left the game
    PlayerLeft,
    /// Enemy sync (from host to clients) - periodic state correction
    EnemySync(EnemySync),
    /// Enemy damage event (from client to host)
    EnemyDamageEvent(EnemyDamage),
    /// Wave start event - shares RNG seed for deterministic spawning
    WaveStartEvent(WaveStart),
    /// Enemy kill event - authoritative from killer's machine
    EnemyKillEvent(EnemyKill),
    /// Batched enemy kill events
    EnemyKillBatchEvent(EnemyKillBatch),
    /// Player death event - authoritative from victim's machine
    PlayerDeathEvent(PlayerDeath),
    /// Chat message
    ChatMessageEvent(ChatMessage),
    /// Vote mute request
    VoteMuteEvent(VoteMute),
    /// Authoritative per-player stats snapshot
    PlayerStatsEvent(PlayerStatsSnapshot),
    /// Paid obstacle placement event
    PaidObstacleEvent(PaidObstacle),
    /// Paid obstacle sync (for late joiners)
    PaidObstacleSyncEvent(PaidObstacleSync),
    /// Paid ability activation event
    PaidAbilityEvent(PaidAbility),
    /// Cannon shot event (host authoritative)
    CannonShotEvent(CannonShot),
    /// Projectile reflection event (authoritative from reflecting player)
    ProjectileReflectionEvent(ProjectileReflection),
    /// Paid obstacle verification ack
    PaidObstacleAckEvent(PaidObstacleAck),
    /// Paid ability verification ack
    PaidAbilityAckEvent(PaidAbilityAck),
    /// Paid name reservation event
    PaidNameReservationEvent(PaidNameReservation),
    /// Paid name reservation sync for late joiners
    PaidNameSyncEvent(PaidNameSync),
    /// Paid name reservation verification ack
    PaidNameAckEvent(PaidNameAck),
    /// Input frame event (future rollback netcode)
    InputFrameEvent(InputFrame),
    /// Latency ping (for supernode selection)
    PingEvent(Ping),
    /// Latency pong (for supernode selection)
    PongEvent(Pong),
    /// Supernode score broadcast
    SupernodeScoreEvent(SupernodeScore),
    /// Batched player state updates (supernode relay)
    PlayerStateBatchEvent(PlayerStateBatch),
    /// Batched input frames (supernode relay)
    InputFrameBatchEvent(InputFrameBatch),
    /// Topology update (room-wide relay tree map from the root)
    TopologyUpdateEvent(TopologyUpdate),
    /// Area authority update (area ownership map)
    AreaAuthorityUpdateEvent(AreaAuthorityUpdate),
    /// Relay-tree membership request, relayed up to the root
    JoinRequestEvent(JoinRequest),
    /// Incremental relay-tree map change (root-originated)
    TopologyDeltaEvent(TopologyDelta),
}

impl NetMessage {
    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            NetMessage::PlayerUpdate(state) => {
                let mut bytes = vec![0u8]; // Message type 0
                bytes.extend_from_slice(&state.to_bytes());
                bytes
            }
            NetMessage::PlayerJoined(name) => {
                let mut bytes = vec![1u8]; // Message type 1
                let name_bytes = name.as_bytes();
                let len = name_bytes.len().min(32) as u8; // Max 32 chars
                bytes.push(len);
                bytes.extend_from_slice(&name_bytes[..len as usize]);
                bytes
            }
            NetMessage::PlayerLeft => vec![2u8],
            NetMessage::EnemySync(sync) => {
                let mut bytes = vec![3u8]; // Message type 3
                bytes.extend_from_slice(&sync.to_bytes());
                bytes
            }
            NetMessage::EnemyDamageEvent(damage) => {
                let mut bytes = vec![4u8]; // Message type 4
                bytes.extend_from_slice(&damage.to_bytes());
                bytes
            }
            NetMessage::WaveStartEvent(wave_start) => {
                let mut bytes = vec![5u8]; // Message type 5
                bytes.extend_from_slice(&wave_start.to_bytes());
                bytes
            }
            NetMessage::EnemyKillEvent(kill) => {
                let mut bytes = vec![6u8]; // Message type 6
                bytes.extend_from_slice(&kill.to_bytes());
                bytes
            }
            NetMessage::EnemyKillBatchEvent(batch) => {
                let mut bytes = vec![29u8]; // Message type 29
                bytes.extend_from_slice(&batch.to_bytes());
                bytes
            }
            NetMessage::PlayerDeathEvent(death) => {
                let mut bytes = vec![7u8]; // Message type 7
                bytes.extend_from_slice(&death.to_bytes());
                bytes
            }
            NetMessage::ChatMessageEvent(chat) => {
                let mut bytes = vec![16u8]; // Message type 16
                bytes.extend_from_slice(&chat.to_bytes());
                bytes
            }
            NetMessage::VoteMuteEvent(vote) => {
                let mut bytes = vec![17u8]; // Message type 17
                bytes.extend_from_slice(&vote.to_bytes());
                bytes
            }
            NetMessage::PlayerStatsEvent(stats) => {
                let mut bytes = vec![27u8]; // Message type 27
                bytes.extend_from_slice(&stats.to_bytes());
                bytes
            }
            NetMessage::PaidObstacleEvent(obstacle) => {
                let mut bytes = vec![8u8]; // Message type 8
                bytes.extend_from_slice(&obstacle.to_bytes());
                bytes
            }
            NetMessage::PaidObstacleSyncEvent(sync) => {
                let mut bytes = vec![9u8]; // Message type 9
                bytes.extend_from_slice(&sync.to_bytes());
                bytes
            }
            NetMessage::PaidAbilityEvent(ability) => {
                let mut bytes = vec![20u8]; // Message type 20
                bytes.extend_from_slice(&ability.to_bytes());
                bytes
            }
            NetMessage::CannonShotEvent(shot) => {
                let mut bytes = vec![10u8]; // Message type 10
                bytes.extend_from_slice(&shot.to_bytes());
                bytes
            }
            NetMessage::ProjectileReflectionEvent(reflection) => {
                let mut bytes = vec![28u8]; // Message type 28
                bytes.extend_from_slice(&reflection.to_bytes());
                bytes
            }
            NetMessage::PaidObstacleAckEvent(ack) => {
                let mut bytes = vec![15u8]; // Message type 15
                bytes.extend_from_slice(&ack.to_bytes());
                bytes
            }
            NetMessage::PaidAbilityAckEvent(ack) => {
                let mut bytes = vec![21u8]; // Message type 21
                bytes.extend_from_slice(&ack.to_bytes());
                bytes
            }
            NetMessage::PaidNameReservationEvent(reservation) => {
                let mut bytes = vec![24u8]; // Message type 24
                bytes.extend_from_slice(&reservation.to_bytes());
                bytes
            }
            NetMessage::PaidNameSyncEvent(sync) => {
                let mut bytes = vec![25u8]; // Message type 25
                bytes.extend_from_slice(&sync.to_bytes());
                bytes
            }
            NetMessage::PaidNameAckEvent(ack) => {
                let mut bytes = vec![26u8]; // Message type 26
                bytes.extend_from_slice(&ack.to_bytes());
                bytes
            }
            NetMessage::InputFrameEvent(frame) => {
                let mut bytes = vec![11u8]; // Message type 11
                bytes.extend_from_slice(&frame.to_bytes());
                bytes
            }
            NetMessage::PingEvent(ping) => {
                let mut bytes = vec![12u8]; // Message type 12
                bytes.extend_from_slice(&ping.to_bytes());
                bytes
            }
            NetMessage::PongEvent(pong) => {
                let mut bytes = vec![13u8]; // Message type 13
                bytes.extend_from_slice(&pong.to_bytes());
                bytes
            }
            NetMessage::SupernodeScoreEvent(score) => {
                let mut bytes = vec![14u8]; // Message type 14
                bytes.extend_from_slice(&score.to_bytes());
                bytes
            }
            NetMessage::PlayerStateBatchEvent(batch) => {
                let mut bytes = vec![18u8]; // Message type 18
                bytes.extend_from_slice(&batch.to_bytes());
                bytes
            }
            NetMessage::InputFrameBatchEvent(batch) => {
                let mut bytes = vec![19u8]; // Message type 19
                bytes.extend_from_slice(&batch.to_bytes());
                bytes
            }
            NetMessage::TopologyUpdateEvent(update) => {
                let mut bytes = vec![22u8]; // Message type 22
                bytes.extend_from_slice(&update.to_bytes());
                bytes
            }
            NetMessage::AreaAuthorityUpdateEvent(update) => {
                let mut bytes = vec![23u8]; // Message type 23
                bytes.extend_from_slice(&update.to_bytes());
                bytes
            }
            NetMessage::JoinRequestEvent(request) => {
                let mut bytes = vec![30u8]; // Message type 30
                bytes.extend_from_slice(&request.to_bytes());
                bytes
            }
            NetMessage::TopologyDeltaEvent(delta) => {
                let mut bytes = vec![31u8]; // Message type 31
                bytes.extend_from_slice(&delta.to_bytes());
                bytes
            }
        }
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.is_empty() {
            return None;
        }
        match bytes[0] {
            0 => PlayerState::from_bytes(&bytes[1..]).map(NetMessage::PlayerUpdate),
            1 => {
                if bytes.len() < 2 {
                    return Some(NetMessage::PlayerJoined("Player".to_string()));
                }
                let len = bytes[1] as usize;
                if bytes.len() < 2 + len {
                    return Some(NetMessage::PlayerJoined("Player".to_string()));
                }
                let name = String::from_utf8_lossy(&bytes[2..2 + len]).to_string();
                Some(NetMessage::PlayerJoined(name))
            }
            2 => Some(NetMessage::PlayerLeft),
            3 => EnemySync::from_bytes(&bytes[1..]).map(NetMessage::EnemySync),
            4 => EnemyDamage::from_bytes(&bytes[1..]).map(NetMessage::EnemyDamageEvent),
            5 => WaveStart::from_bytes(&bytes[1..]).map(NetMessage::WaveStartEvent),
            6 => EnemyKill::from_bytes(&bytes[1..]).map(NetMessage::EnemyKillEvent),
            29 => EnemyKillBatch::from_bytes(&bytes[1..]).map(NetMessage::EnemyKillBatchEvent),
            7 => PlayerDeath::from_bytes(&bytes[1..]).map(NetMessage::PlayerDeathEvent),
            16 => ChatMessage::from_bytes(&bytes[1..]).map(NetMessage::ChatMessageEvent),
            17 => VoteMute::from_bytes(&bytes[1..]).map(NetMessage::VoteMuteEvent),
            27 => PlayerStatsSnapshot::from_bytes(&bytes[1..]).map(NetMessage::PlayerStatsEvent),
            8 => PaidObstacle::from_bytes(&bytes[1..]).map(NetMessage::PaidObstacleEvent),
            9 => PaidObstacleSync::from_bytes(&bytes[1..]).map(NetMessage::PaidObstacleSyncEvent),
            10 => CannonShot::from_bytes(&bytes[1..]).map(NetMessage::CannonShotEvent),
            28 => ProjectileReflection::from_bytes(&bytes[1..])
                .map(NetMessage::ProjectileReflectionEvent),
            20 => PaidAbility::from_bytes(&bytes[1..]).map(NetMessage::PaidAbilityEvent),
            11 => InputFrame::from_bytes(&bytes[1..]).map(NetMessage::InputFrameEvent),
            12 => Ping::from_bytes(&bytes[1..]).map(NetMessage::PingEvent),
            13 => Pong::from_bytes(&bytes[1..]).map(NetMessage::PongEvent),
            14 => SupernodeScore::from_bytes(&bytes[1..]).map(NetMessage::SupernodeScoreEvent),
            15 => PaidObstacleAck::from_bytes(&bytes[1..]).map(NetMessage::PaidObstacleAckEvent),
            21 => PaidAbilityAck::from_bytes(&bytes[1..]).map(NetMessage::PaidAbilityAckEvent),
            24 => PaidNameReservation::from_bytes(&bytes[1..])
                .map(NetMessage::PaidNameReservationEvent),
            25 => PaidNameSync::from_bytes(&bytes[1..]).map(NetMessage::PaidNameSyncEvent),
            26 => PaidNameAck::from_bytes(&bytes[1..]).map(NetMessage::PaidNameAckEvent),
            18 => PlayerStateBatch::from_bytes(&bytes[1..]).map(NetMessage::PlayerStateBatchEvent),
            19 => InputFrameBatch::from_bytes(&bytes[1..]).map(NetMessage::InputFrameBatchEvent),
            22 => TopologyUpdate::from_bytes(&bytes[1..]).map(NetMessage::TopologyUpdateEvent),
            23 => AreaAuthorityUpdate::from_bytes(&bytes[1..])
                .map(NetMessage::AreaAuthorityUpdateEvent),
            30 => JoinRequest::from_bytes(&bytes[1..]).map(NetMessage::JoinRequestEvent),
            31 => TopologyDelta::from_bytes(&bytes[1..]).map(NetMessage::TopologyDeltaEvent),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_frame_roundtrip_u16() {
        let frame = InputFrame {
            frame: 42,
            input: 0b1010_1010_1111_0000,
        };
        let bytes = frame.to_bytes();
        let decoded = InputFrame::from_bytes(&bytes).expect("decode");
        assert_eq!(decoded.frame, frame.frame);
        assert_eq!(decoded.input, frame.input);
    }

    #[test]
    fn topology_map_roundtrip() {
        let update = TopologyUpdate {
            epoch: 42,
            root_hash: 0xDEAD_BEEF_CAFE_F00D,
            fanout: 6,
            entries: vec![
                TopologyEntry {
                    peer_hash: 0xDEAD_BEEF_CAFE_F00D,
                    uuid: [1u8; 16],
                    parent_hash: 0,
                },
                TopologyEntry {
                    peer_hash: 7,
                    uuid: [2u8; 16],
                    parent_hash: 0xDEAD_BEEF_CAFE_F00D,
                },
            ],
        };
        let bytes = NetMessage::TopologyUpdateEvent(update.clone()).to_bytes();
        let decoded = NetMessage::from_bytes(&bytes).expect("decode");
        match decoded {
            NetMessage::TopologyUpdateEvent(decoded) => assert_eq!(decoded, update),
            other => panic!("wrong message type: {other:?}"),
        }
    }

    #[test]
    fn topology_delta_roundtrip() {
        let delta = TopologyDelta {
            epoch_from: 7,
            epoch_to: 9,
            root_hash: 0xAA,
            fanout: 8,
            checksum: 0x1234_5678,
            removed: vec![1, 2],
            upserts: vec![TopologyEntry {
                peer_hash: 3,
                uuid: [4u8; 16],
                parent_hash: 0xAA,
            }],
        };
        let bytes = NetMessage::TopologyDeltaEvent(delta.clone()).to_bytes();
        match NetMessage::from_bytes(&bytes).expect("decode") {
            NetMessage::TopologyDeltaEvent(decoded) => assert_eq!(decoded, delta),
            other => panic!("wrong message type: {other:?}"),
        }
    }

    #[test]
    fn join_request_roundtrip() {
        let request = JoinRequest {
            peer_hash: 0x1234_5678_9ABC_DEF0,
            uuid: [9u8; 16],
        };
        let bytes = NetMessage::JoinRequestEvent(request).to_bytes();
        let decoded = NetMessage::from_bytes(&bytes).expect("decode");
        match decoded {
            NetMessage::JoinRequestEvent(decoded) => assert_eq!(decoded, request),
            other => panic!("wrong message type: {other:?}"),
        }
    }

    #[test]
    fn cannon_shot_roundtrip_with_id() {
        let shot = CannonShot {
            id: 77,
            x: 12.5,
            y: -9.25,
            vx: 1.75,
            vy: -2.5,
        };
        let bytes = shot.to_bytes();
        let decoded = CannonShot::from_bytes(&bytes).expect("decode");
        assert_eq!(decoded.id, shot.id);
        assert_eq!(decoded.x, shot.x);
        assert_eq!(decoded.y, shot.y);
        assert_eq!(decoded.vx, shot.vx);
        assert_eq!(decoded.vy, shot.vy);
    }

    #[test]
    fn projectile_reflection_roundtrip() {
        let reflection = ProjectileReflection {
            id: 91,
            x: -11.0,
            y: 4.5,
            vx: -3.0,
            vy: 2.0,
        };
        let bytes = reflection.to_bytes();
        let decoded = ProjectileReflection::from_bytes(&bytes).expect("decode");
        assert_eq!(decoded.id, reflection.id);
        assert_eq!(decoded.x, reflection.x);
        assert_eq!(decoded.y, reflection.y);
        assert_eq!(decoded.vx, reflection.vx);
        assert_eq!(decoded.vy, reflection.vy);
    }

    #[test]
    fn enemy_kill_batch_roundtrip() {
        let kills = vec![
            EnemyKill {
                enemy_type: 0,
                enemy_id: 3,
                killer_x: 10.0,
                killer_y: -4.0,
                killer_hash: 12,
                event_id: 1001,
            },
            EnemyKill {
                enemy_type: 2,
                enemy_id: 9,
                killer_x: 7.5,
                killer_y: 6.0,
                killer_hash: 12,
                event_id: 1002,
            },
        ];
        let batch = EnemyKillBatch {
            kills: kills.clone(),
        };
        let bytes = batch.to_bytes();
        let decoded = EnemyKillBatch::from_bytes(&bytes).expect("decode");
        assert_eq!(decoded.kills.len(), kills.len());
        assert_eq!(decoded.kills[0].event_id, kills[0].event_id);
        assert_eq!(decoded.kills[1].event_id, kills[1].event_id);
    }

    #[test]
    fn paid_ability_roundtrip() {
        let mut hash = [0u8; 32];
        hash[0] = 3;
        let ability = PaidAbility {
            ability_type: PaidAbilityType::Shockwave as u8,
            x: 12.5,
            y: -9.25,
            radius: 70.0,
            nonce: 777,
            proof_hash: hash,
        };
        let bytes = ability.to_bytes();
        let decoded = PaidAbility::from_bytes(&bytes).expect("decode");
        assert_eq!(decoded.ability_type, ability.ability_type);
        assert_eq!(decoded.x, ability.x);
        assert_eq!(decoded.y, ability.y);
        assert_eq!(decoded.radius, ability.radius);
        assert_eq!(decoded.nonce, ability.nonce);
        assert_eq!(decoded.proof_hash, ability.proof_hash);
    }

    #[test]
    fn paid_name_roundtrip() {
        let mut hash = [0u8; 32];
        hash[0] = 7;
        let reservation = PaidNameReservation::from_name(42, "KEYSLIME", 99, hash);
        let bytes = reservation.to_bytes();
        let decoded = PaidNameReservation::from_bytes(&bytes).expect("decode");
        assert_eq!(decoded.owner_hash, 42);
        assert_eq!(decoded.nonce, 99);
        assert_eq!(decoded.name_string(), "KEYSLIME".to_string());
        assert_eq!(decoded.proof_hash, hash);
    }
}

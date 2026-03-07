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
    pub id: u16,           // Enemy ID (2 bytes)
    pub enemy_type: u8,    // EnemyType (1 byte)
    pub flags: u8,         // alive flag + other bits (1 byte)
    pub x: f32,            // Position X (4 bytes)
    pub y: f32,            // Position Y (4 bytes)
    pub dir_x: f32,        // Direction/look_dir X (4 bytes)
    pub dir_y: f32,        // Direction/look_dir Y (4 bytes)
    pub extra: u8,         // Snake size / cannon shoot timer low bits (1 byte)
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
    pub wave: u32,
    pub enemies: Vec<EnemyState>,
}

impl EnemySync {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&self.wave.to_le_bytes());
        bytes.extend_from_slice(&(self.enemies.len() as u16).to_le_bytes());
        for enemy in &self.enemies {
            bytes.extend_from_slice(&enemy.to_bytes());
        }
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 6 {
            return None;
        }
        let wave = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;

        let mut enemies = Vec::with_capacity(count);
        let mut offset = 6;
        for _ in 0..count {
            if offset + 21 > bytes.len() {
                break;
            }
            if let Some(enemy) = EnemyState::from_bytes(&bytes[offset..]) {
                enemies.push(enemy);
            }
            offset += 21;
        }

        Some(Self { wave, enemies })
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
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
}

impl CannonShot {
    pub fn to_bytes(&self) -> [u8; 16] {
        let mut bytes = [0u8; 16];
        bytes[0..4].copy_from_slice(&self.x.to_le_bytes());
        bytes[4..8].copy_from_slice(&self.y.to_le_bytes());
        bytes[8..12].copy_from_slice(&self.vx.to_le_bytes());
        bytes[12..16].copy_from_slice(&self.vy.to_le_bytes());
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 16 {
            return None;
        }
        Some(Self {
            x: f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            y: f32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
            vx: f32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
            vy: f32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]),
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
            rng_seed: u64::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7], bytes[8], bytes[9], bytes[10], bytes[11]]),
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
    pub killer_x: f32,  // Position where kill happened (for verification)
    pub killer_y: f32,
    pub killer_hash: u64,
    pub event_id: u64,
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
                bytes[11], bytes[12], bytes[13], bytes[14],
                bytes[15], bytes[16], bytes[17], bytes[18],
            ]),
            event_id: u64::from_le_bytes([
                bytes[19], bytes[20], bytes[21], bytes[22],
                bytes[23], bytes[24], bytes[25], bytes[26],
            ]),
        })
    }
}

/// Player death event - authoritative from the victim's machine
#[derive(Debug, Clone, Copy)]
pub struct PlayerDeath {
    pub death_x: f32,
    pub death_y: f32,
    pub killed_by_type: u8,  // 0=spider, 1=cannon, 2=snake, 3=projectile, 4=wisp
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
            bytes[0], bytes[1], bytes[2], bytes[3],
            bytes[4], bytes[5], bytes[6], bytes[7],
        ]);
        let len = bytes[8] as usize;
        if bytes.len() < 9 + len {
            return None;
        }
        let text = String::from_utf8(bytes[9..9 + len].to_vec()).ok()?;
        Some(Self { sender_hash: hash, text })
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
                bytes[0], bytes[1], bytes[2], bytes[3],
                bytes[4], bytes[5], bytes[6], bytes[7],
            ]),
            voter_hash: u64::from_le_bytes([
                bytes[8], bytes[9], bytes[10], bytes[11],
                bytes[12], bytes[13], bytes[14], bytes[15],
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
                bytes[11], bytes[12], bytes[13], bytes[14],
                bytes[15], bytes[16], bytes[17], bytes[18],
            ]),
            event_id: u64::from_le_bytes([
                bytes[19], bytes[20], bytes[21], bytes[22],
                bytes[23], bytes[24], bytes[25], bytes[26],
            ]),
        })
    }
}

// ============ Player State ============

/// Compact player state for network transmission (25 bytes)
#[derive(Debug, Clone, Copy)]
pub struct PlayerState {
    pub x: f32,
    pub y: f32,
    pub look_dir_x: f32,
    pub look_dir_y: f32,
    pub move_dir_x: f32,  // For tail animation
    pub move_dir_y: f32,
    pub flags: u8, // alive|attacking|blocking|phasing|shielded|_|_|_
}

impl PlayerState {
    pub fn new(
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
            x: pos.x,
            y: pos.y,
            look_dir_x: look_dir.x,
            look_dir_y: look_dir.y,
            move_dir_x: move_dir.x,
            move_dir_y: move_dir.y,
            flags,
        }
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

    pub fn to_bytes(&self) -> [u8; 25] {
        let mut bytes = [0u8; 25];
        bytes[0..4].copy_from_slice(&self.x.to_le_bytes());
        bytes[4..8].copy_from_slice(&self.y.to_le_bytes());
        bytes[8..12].copy_from_slice(&self.look_dir_x.to_le_bytes());
        bytes[12..16].copy_from_slice(&self.look_dir_y.to_le_bytes());
        bytes[16..20].copy_from_slice(&self.move_dir_x.to_le_bytes());
        bytes[20..24].copy_from_slice(&self.move_dir_y.to_le_bytes());
        bytes[24] = self.flags;
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 25 {
            return None;
        }
        Some(Self {
            x: f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            y: f32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
            look_dir_x: f32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
            look_dir_y: f32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]),
            move_dir_x: f32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]),
            move_dir_y: f32::from_le_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]),
            flags: bytes[24],
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
            if offset + 8 + 4 + 25 > bytes.len() {
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
            let state = PlayerState::from_bytes(&bytes[offset..offset + 25])?;
            offset += 25;
            entries.push(PlayerStateEntry { peer_hash, area_id, state });
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
            entries.push(InputFrameEntry { peer_hash, area_id, frame });
        }
        Some(Self { entries })
    }
}

#[derive(Debug, Clone)]
pub struct TopologyUpdate {
    pub epoch: u32,
    pub super_root_hash: u64,
    pub supernode_hashes: Vec<u64>,
    pub fanout: u8,
    pub parent_hash: u64,
    pub backup_parent_hash: u64,
    pub child_hashes: Vec<u64>,
}

impl TopologyUpdate {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&self.epoch.to_le_bytes());
        bytes.extend_from_slice(&self.super_root_hash.to_le_bytes());
        bytes.extend_from_slice(&(self.supernode_hashes.len() as u16).to_le_bytes());
        for hash in &self.supernode_hashes {
            bytes.extend_from_slice(&hash.to_le_bytes());
        }
        bytes.push(self.fanout);
        bytes.extend_from_slice(&self.parent_hash.to_le_bytes());
        bytes.extend_from_slice(&self.backup_parent_hash.to_le_bytes());
        bytes.extend_from_slice(&(self.child_hashes.len() as u16).to_le_bytes());
        for hash in &self.child_hashes {
            bytes.extend_from_slice(&hash.to_le_bytes());
        }
        bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < 4 + 8 + 2 + 1 + 8 + 8 + 2 {
            return None;
        }
        let epoch = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let super_root_hash = u64::from_le_bytes([
            bytes[4], bytes[5], bytes[6], bytes[7],
            bytes[8], bytes[9], bytes[10], bytes[11],
        ]);
        let mut offset = 12;
        let super_count = u16::from_le_bytes([bytes[offset], bytes[offset + 1]]) as usize;
        offset += 2;
        let mut supernode_hashes = Vec::with_capacity(super_count);
        for _ in 0..super_count {
            if offset + 8 > bytes.len() {
                return None;
            }
            supernode_hashes.push(u64::from_le_bytes([
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
                bytes[offset + 4],
                bytes[offset + 5],
                bytes[offset + 6],
                bytes[offset + 7],
            ]));
            offset += 8;
        }
        if offset + 1 + 8 + 8 + 2 > bytes.len() {
            return None;
        }
        let fanout = bytes[offset];
        offset += 1;
        let parent_hash = u64::from_le_bytes([
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
        let backup_parent_hash = u64::from_le_bytes([
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
        let child_count = u16::from_le_bytes([bytes[offset], bytes[offset + 1]]) as usize;
        offset += 2;
        let mut child_hashes = Vec::with_capacity(child_count);
        for _ in 0..child_count {
            if offset + 8 > bytes.len() {
                return None;
            }
            child_hashes.push(u64::from_le_bytes([
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
                bytes[offset + 4],
                bytes[offset + 5],
                bytes[offset + 6],
                bytes[offset + 7],
            ]));
            offset += 8;
        }
        Some(Self {
            epoch,
            super_root_hash,
            supernode_hashes,
            fanout,
            parent_hash,
            backup_parent_hash,
            child_hashes,
        })
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
            entries.push(AreaAuthorityEntry { area_id, authority_hash });
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
    /// Player death event - authoritative from victim's machine
    PlayerDeathEvent(PlayerDeath),
    /// Chat message
    ChatMessageEvent(ChatMessage),
    /// Vote mute request
    VoteMuteEvent(VoteMute),
    /// Paid obstacle placement event
    PaidObstacleEvent(PaidObstacle),
    /// Paid obstacle sync (for late joiners)
    PaidObstacleSyncEvent(PaidObstacleSync),
    /// Paid ability activation event
    PaidAbilityEvent(PaidAbility),
    /// Cannon shot event (host authoritative)
    CannonShotEvent(CannonShot),
    /// Paid obstacle verification ack
    PaidObstacleAckEvent(PaidObstacleAck),
    /// Paid ability verification ack
    PaidAbilityAckEvent(PaidAbilityAck),
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
    /// Topology update (multi-supernode relay tree)
    TopologyUpdateEvent(TopologyUpdate),
    /// Area authority update (area ownership map)
    AreaAuthorityUpdateEvent(AreaAuthorityUpdate),
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
            7 => PlayerDeath::from_bytes(&bytes[1..]).map(NetMessage::PlayerDeathEvent),
            16 => ChatMessage::from_bytes(&bytes[1..]).map(NetMessage::ChatMessageEvent),
            17 => VoteMute::from_bytes(&bytes[1..]).map(NetMessage::VoteMuteEvent),
            8 => PaidObstacle::from_bytes(&bytes[1..]).map(NetMessage::PaidObstacleEvent),
            9 => PaidObstacleSync::from_bytes(&bytes[1..]).map(NetMessage::PaidObstacleSyncEvent),
            10 => CannonShot::from_bytes(&bytes[1..]).map(NetMessage::CannonShotEvent),
            20 => PaidAbility::from_bytes(&bytes[1..]).map(NetMessage::PaidAbilityEvent),
            11 => InputFrame::from_bytes(&bytes[1..]).map(NetMessage::InputFrameEvent),
            12 => Ping::from_bytes(&bytes[1..]).map(NetMessage::PingEvent),
            13 => Pong::from_bytes(&bytes[1..]).map(NetMessage::PongEvent),
            14 => SupernodeScore::from_bytes(&bytes[1..]).map(NetMessage::SupernodeScoreEvent),
            15 => PaidObstacleAck::from_bytes(&bytes[1..]).map(NetMessage::PaidObstacleAckEvent),
            21 => PaidAbilityAck::from_bytes(&bytes[1..]).map(NetMessage::PaidAbilityAckEvent),
            18 => PlayerStateBatch::from_bytes(&bytes[1..]).map(NetMessage::PlayerStateBatchEvent),
            19 => InputFrameBatch::from_bytes(&bytes[1..]).map(NetMessage::InputFrameBatchEvent),
            22 => TopologyUpdate::from_bytes(&bytes[1..]).map(NetMessage::TopologyUpdateEvent),
            23 => AreaAuthorityUpdate::from_bytes(&bytes[1..]).map(NetMessage::AreaAuthorityUpdateEvent),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_frame_roundtrip_u16() {
        let frame = InputFrame { frame: 42, input: 0b1010_1010_1111_0000 };
        let bytes = frame.to_bytes();
        let decoded = InputFrame::from_bytes(&bytes).expect("decode");
        assert_eq!(decoded.frame, frame.frame);
        assert_eq!(decoded.input, frame.input);
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
}

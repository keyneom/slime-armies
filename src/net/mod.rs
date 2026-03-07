mod session;
mod protocol;
mod remote_player;

pub use session::{IceConfig, NetworkSession, NetworkState, PlayerStats};
pub use protocol::{
    NetMessage, PlayerState, EnemyState, EnemySync, EnemyDamage, EnemyType, WaveStart, EnemyKill,
    PlayerDeath, PaidObstacle, PaidObstacleSync, PaidObstacleAck, PaidAbility, PaidAbilityAck, PaidAbilityType,
    CannonShot, InputFrame, Ping, Pong, SupernodeScore, ChatMessage, VoteMute, PlayerStateBatch,
    PlayerStateEntry, InputFrameBatch, InputFrameEntry, TopologyUpdate, AreaAuthorityUpdate, AreaAuthorityEntry,
};
pub use remote_player::RemotePlayer;

// Re-export for future use
#[allow(unused_imports)]
pub use session::PeerId;

mod protocol;
mod remote_player;
mod session;

pub use protocol::{
    AreaAuthorityEntry, AreaAuthorityUpdate, CannonShot, ChatMessage, EnemyDamage, EnemyKill,
    EnemyState, EnemySync, EnemyType, InputFrame, InputFrameBatch, InputFrameEntry, NetMessage,
    PaidAbility, PaidAbilityAck, PaidAbilityType, PaidNameAck, PaidNameReservation, PaidNameSync,
    PaidObstacle, PaidObstacleAck, PaidObstacleSync, Ping, PlayerDeath, PlayerState,
    PlayerStateBatch, PlayerStateEntry, PlayerStatsSnapshot, Pong, ProjectileReflection,
    SupernodeScore, TopologyUpdate, VoteMute, WaveStart,
};
pub use remote_player::RemotePlayer;
pub use session::{IceConfig, NetworkSession, NetworkState, PlayerStats};

// Re-export for future use
#[allow(unused_imports)]
pub use session::PeerId;

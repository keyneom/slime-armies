mod session;
mod protocol;
mod remote_player;

pub use session::{NetworkSession, NetworkState, PlayerStats};
pub use protocol::{NetMessage, PlayerState, EnemyState, EnemySync, EnemyDamage, EnemyType, WaveStart, EnemyKill, PlayerDeath, PaidObstacle, PaidObstacleSync, PaidObstacleAck, CannonShot, InputFrame, Ping, Pong, SupernodeScore, ChatMessage, VoteMute};
pub use remote_player::RemotePlayer;

// Re-export for future use
#[allow(unused_imports)]
pub use session::PeerId;

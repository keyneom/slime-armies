mod camera;
mod chunk;
mod noise;

pub use camera::Camera;
pub use chunk::ChunkManager;

// Re-export for future use
#[allow(unused_imports)]
pub use chunk::{Chunk, Obstacle, CHUNK_SIZE};
#[allow(unused_imports)]
pub use noise::SimplexNoise;

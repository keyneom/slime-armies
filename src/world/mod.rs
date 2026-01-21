mod chunk;
mod noise;
mod camera;

pub use chunk::ChunkManager;
pub use camera::Camera;

// Re-export for future use
#[allow(unused_imports)]
pub use chunk::{Chunk, Obstacle, CHUNK_SIZE};
#[allow(unused_imports)]
pub use noise::SimplexNoise;

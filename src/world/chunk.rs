use crate::math::Vec2;
use crate::world::noise::SimplexNoise;
use std::collections::{HashMap, HashSet};

pub const CHUNK_SIZE: i32 = 512;

/// Terrain type for a tile
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Terrain {
    Empty,
    Grass,
    Rock,      // Obstacle - blocks movement
    Water,     // Slows movement (future)
}

/// A single chunk of the infinite world
#[derive(Clone)]
pub struct Chunk {
    pub x: i32,
    pub y: i32,
    pub terrain: Vec<Terrain>,
    pub obstacles: Vec<Obstacle>,
    pub enemy_spawn_seed: u64,
}

#[derive(Debug, Clone)]
pub struct Obstacle {
    pub pos: Vec2,
    pub radius: f32,
    pub variant: u8,  // Visual variant
}

impl Chunk {
    pub fn new(x: i32, y: i32, world_seed: u64, noise: &SimplexNoise) -> Self {
        let chunk_seed = Self::hash_coords(x, y, world_seed);
        let mut terrain = vec![Terrain::Empty; (CHUNK_SIZE * CHUNK_SIZE / 64) as usize];
        let mut obstacles = Vec::new();

        // Generate terrain using noise
        let scale = 0.01;
        let obstacle_scale = 0.05;

        // Sample terrain at a lower resolution (8x8 pixel tiles)
        let tile_size = 8;
        let tiles_per_side = CHUNK_SIZE / tile_size;

        for ty in 0..tiles_per_side {
            for tx in 0..tiles_per_side {
                let world_x = (x * CHUNK_SIZE + tx * tile_size) as f64;
                let world_y = (y * CHUNK_SIZE + ty * tile_size) as f64;

                // Use FBM for natural-looking terrain
                let value = noise.fbm(world_x * scale, world_y * scale, 4, 0.5, 2.0);

                let terrain_type = if value > 0.4 {
                    Terrain::Rock
                } else if value > -0.3 {
                    Terrain::Grass
                } else {
                    Terrain::Empty
                };

                let idx = (ty * tiles_per_side + tx) as usize;
                if idx < terrain.len() {
                    terrain[idx] = terrain_type;
                }
            }
        }

        // Generate obstacles (rocks, trees) using different noise frequency
        let mut rng_state = chunk_seed;
        let obstacle_count = 5 + (Self::next_random(&mut rng_state) % 10) as usize;

        for _ in 0..obstacle_count {
            let local_x = (Self::next_random(&mut rng_state) % CHUNK_SIZE as u64) as f32;
            let local_y = (Self::next_random(&mut rng_state) % CHUNK_SIZE as u64) as f32;

            let world_x = (x * CHUNK_SIZE) as f32 + local_x;
            let world_y = (y * CHUNK_SIZE) as f32 + local_y;

            // Use noise to determine if obstacle should exist here
            let obstacle_value = noise.noise2d(
                world_x as f64 * obstacle_scale,
                world_y as f64 * obstacle_scale,
            );

            if obstacle_value > 0.3 {
                obstacles.push(Obstacle {
                    pos: Vec2::new(world_x, world_y),
                    radius: 15.0 + (Self::next_random(&mut rng_state) % 20) as f32,
                    variant: (Self::next_random(&mut rng_state) % 3) as u8,
                });
            }
        }

        Self {
            x,
            y,
            terrain,
            obstacles,
            enemy_spawn_seed: chunk_seed,
        }
    }

    fn hash_coords(x: i32, y: i32, seed: u64) -> u64 {
        let mut hash = seed;
        hash = hash.wrapping_mul(0x517cc1b727220a95);
        hash ^= x as u64;
        hash = hash.wrapping_mul(0x517cc1b727220a95);
        hash ^= y as u64;
        hash = hash.wrapping_mul(0x517cc1b727220a95);
        hash
    }

    fn next_random(state: &mut u64) -> u64 {
        *state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        *state >> 33
    }

    /// Get world position of chunk's top-left corner
    pub fn world_pos(&self) -> Vec2 {
        Vec2::new(
            (self.x * CHUNK_SIZE) as f32,
            (self.y * CHUNK_SIZE) as f32,
        )
    }

    /// Check if a point collides with any obstacle in this chunk
    pub fn collides_with_obstacle(&self, pos: Vec2, radius: f32) -> bool {
        for obstacle in &self.obstacles {
            if pos.distance(obstacle.pos) < radius + obstacle.radius {
                return true;
            }
        }
        false
    }
}

/// Manages loading/unloading chunks around the player
pub struct ChunkManager {
    pub chunks: HashMap<(i32, i32), Chunk>,
    dynamic_obstacles: HashMap<(i32, i32), Vec<Obstacle>>,
    pub world_seed: u64,
    noise: SimplexNoise,
    load_radius: i32,
}

impl ChunkManager {
    pub fn new(world_seed: u64) -> Self {
        Self {
            chunks: HashMap::new(),
            dynamic_obstacles: HashMap::new(),
            world_seed,
            noise: SimplexNoise::new(world_seed),
            load_radius: 2, // Load 5x5 chunks around player
        }
    }

    pub fn add_dynamic_obstacle(&mut self, obstacle: Obstacle) {
        let cx = (obstacle.pos.x / CHUNK_SIZE as f32).floor() as i32;
        let cy = (obstacle.pos.y / CHUNK_SIZE as f32).floor() as i32;
        self.dynamic_obstacles.entry((cx, cy)).or_default().push(obstacle.clone());
        if let Some(chunk) = self.chunks.get_mut(&(cx, cy)) {
            chunk.obstacles.push(obstacle);
        }
    }

    pub fn remove_dynamic_obstacle(&mut self, obstacle: &Obstacle) {
        let cx = (obstacle.pos.x / CHUNK_SIZE as f32).floor() as i32;
        let cy = (obstacle.pos.y / CHUNK_SIZE as f32).floor() as i32;

        if let Some(list) = self.dynamic_obstacles.get_mut(&(cx, cy)) {
            list.retain(|entry| !Self::obstacle_matches(entry, obstacle));
        }

        if let Some(chunk) = self.chunks.get_mut(&(cx, cy)) {
            chunk.obstacles.retain(|entry| !Self::obstacle_matches(entry, obstacle));
        }
    }

    fn obstacle_matches(a: &Obstacle, b: &Obstacle) -> bool {
        a.variant == b.variant
            && (a.radius - b.radius).abs() < 0.01
            && a.pos.distance(b.pos) < 0.01
    }

    /// Update loaded chunks based on player position
    pub fn update(&mut self, player_pos: Vec2) {
        self.update_for_positions(&[player_pos]);
    }

    /// Update loaded chunks based on multiple positions (host uses all players)
    pub fn update_for_positions(&mut self, positions: &[Vec2]) {
        if positions.is_empty() {
            return;
        }

        let mut keep_chunks: HashSet<(i32, i32)> = HashSet::new();
        let mut load_chunks: HashSet<(i32, i32)> = HashSet::new();

        let unload_radius = self.load_radius + 2;

        for pos in positions {
            let chunk_x = (pos.x / CHUNK_SIZE as f32).floor() as i32;
            let chunk_y = (pos.y / CHUNK_SIZE as f32).floor() as i32;

            for dy in -self.load_radius..=self.load_radius {
                for dx in -self.load_radius..=self.load_radius {
                    load_chunks.insert((chunk_x + dx, chunk_y + dy));
                }
            }

            for dy in -unload_radius..=unload_radius {
                for dx in -unload_radius..=unload_radius {
                    keep_chunks.insert((chunk_x + dx, chunk_y + dy));
                }
            }
        }

        for (cx, cy) in load_chunks {
            if !self.chunks.contains_key(&(cx, cy)) {
                let mut chunk = Chunk::new(cx, cy, self.world_seed, &self.noise);
                if let Some(extra) = self.dynamic_obstacles.get(&(cx, cy)) {
                    chunk.obstacles.extend_from_slice(extra);
                }
                self.chunks.insert((cx, cy), chunk);
            }
        }

        self.chunks.retain(|&(cx, cy), _| keep_chunks.contains(&(cx, cy)));
    }

    /// Get chunk at world position
    pub fn get_chunk_at(&self, world_pos: Vec2) -> Option<&Chunk> {
        let cx = (world_pos.x / CHUNK_SIZE as f32).floor() as i32;
        let cy = (world_pos.y / CHUNK_SIZE as f32).floor() as i32;
        self.chunks.get(&(cx, cy))
    }

    /// Check collision with any obstacle
    pub fn collides_with_obstacle(&self, pos: Vec2, radius: f32) -> bool {
        // Check current chunk and neighbors
        let cx = (pos.x / CHUNK_SIZE as f32).floor() as i32;
        let cy = (pos.y / CHUNK_SIZE as f32).floor() as i32;

        for dy in -1..=1 {
            for dx in -1..=1 {
                if let Some(chunk) = self.chunks.get(&(cx + dx, cy + dy)) {
                    if chunk.collides_with_obstacle(pos, radius) {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Get all visible obstacles for rendering
    pub fn visible_obstacles(&self, min_x: f32, min_y: f32, max_x: f32, max_y: f32) -> Vec<&Obstacle> {
        let mut result = Vec::new();

        let min_cx = (min_x / CHUNK_SIZE as f32).floor() as i32 - 1;
        let min_cy = (min_y / CHUNK_SIZE as f32).floor() as i32 - 1;
        let max_cx = (max_x / CHUNK_SIZE as f32).ceil() as i32 + 1;
        let max_cy = (max_y / CHUNK_SIZE as f32).ceil() as i32 + 1;

        for cy in min_cy..=max_cy {
            for cx in min_cx..=max_cx {
                if let Some(chunk) = self.chunks.get(&(cx, cy)) {
                    for obstacle in &chunk.obstacles {
                        if obstacle.pos.x + obstacle.radius >= min_x
                            && obstacle.pos.x - obstacle.radius <= max_x
                            && obstacle.pos.y + obstacle.radius >= min_y
                            && obstacle.pos.y - obstacle.radius <= max_y
                        {
                            result.push(obstacle);
                        }
                    }
                }
            }
        }

        result
    }
}

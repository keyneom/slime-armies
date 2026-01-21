# Slime Armies MMO - Progress Tracker

## Original Objective
Transform "One Slime Army" from a single-player WASM-4 game into a P2P multiplayer game with:
- Faithful recreation of the original game's visual style
- P2P multiplayer using WebRTC (Matchbox)
- Infinite procedurally generated world
- Wave-based enemy spawning scaled by player count

## Current Session - Enemy Sync Fix

### Issue: Enemies Out of Sync Between Players
**Problem**: Enemies appeared in different positions for different players because:
- Deterministic spawning requires both players to spawn at exact same frame with same position
- Players have different positions and frame counts, so enemies spawn differently
- The screenshot showed two players with completely different enemy positions

**Solution**: Implemented **Authoritative Host Model** (industry standard for P2P games):
1. **Host runs enemy AI** - only the host simulates enemy movement
2. **Host sends positions continuously** - every 6 frames (~10 updates/sec)
3. **Clients do NOT run enemy AI** - they just receive positions from host
4. **All players still run local collision** - for responsive gameplay (hits feel instant)
5. **Only host spawns new waves** - clients receive wave start events

### Files Modified
- `src/lib.rs:612-630` - Host sends enemy sync every 6 frames, clients always apply
- `src/game.rs:292-313` - `update_game_multiplayer()` now uses `is_host` to control enemy AI
- `src/game.rs:553-563` - Only host checks for wave clear and spawns new waves

### How It Works Now
```
HOST (room creator):
  - Runs enemy AI (movement, targeting)
  - Sends enemy positions every 6 frames
  - Spawns new waves when enemies are cleared
  - Broadcasts wave start events

CLIENT (joiner):
  - Does NOT run enemy AI
  - Receives enemy positions from host
  - Applies positions directly (smooth via interpolation)
  - Receives wave start events and spawns enemies
  - Still runs local collision for responsive hits
```

## All Completed This Session
- [x] Fixed enemy state sync after player death (respawn keeps enemies)
- [x] Added death count tracking per player
- [x] Fixed kill attribution (only killer gets credit)
- [x] Added move_dir to PlayerState for tail animation sync
- [x] Increased movement speeds
- [x] Fixed spider leg rendering (scaled up, proper animation)
- [x] Fixed wave formula to match original * player_count
- [x] **Implemented authoritative host model for enemy sync**
- [x] Implemented pixelated render pipeline (offscreen buffer with UI overlay; `RENDER_SCALE=1.0`)
- [x] Scaled creature rendering + collision consistently (`CREATURE_SCALE=2.0`)
- [x] Tunneling visuals: outline-only body and tail; tail outline masked outside body
- [x] World-wide minimap with zoomable tactical overlay and teleport targeting
- [x] Paid obstacle network plumbing (sync + late joiner state), pending on-chain verification
- [x] Rollback netcode (player movement): input frames + 6-frame rollback window with resimulation
- [x] Supernode relay for authoritative events (kills, deaths, paid obstacles)
- [~] Supernode election scaffolding: latency pings + score broadcast + deterministic fallback (stable before full score exchange, RTT sample floor)
- [~] Supernode failover + paid obstacle ack plumbing (2-of-n confirmations tracked)
- [x] Cannon AI fires when visible to any player (not just host view)
- [x] Wave spawn uses host center coordinates + enemy sync includes dead enemies to keep clients aligned
- [x] Supernode change now re-broadcasts wave/enemy/paid state for resync
- [x] Wave progression based on team kill target; enemies spawn per newly explored chunks across players
- [x] Wave targets tracked per enemy type; spawn density tied to screen-area exploration
- [x] World generation seed derives from room id for deterministic chunks

## Build & Test
```bash
trunk serve  # Dev server on http://localhost:8080
```

## Testing Steps
1. Open two browser tabs to localhost:8080
2. Create a room in first tab (host)
3. Join the room in second tab (client)
4. Both players should see enemies in the SAME positions
5. Host player's attacks should kill enemies visible to both
6. Client player's attacks should also kill enemies (local collision still works)

## Requested Features
- [x] Team scoring system that accounts for kills, deaths, and time played.
  - [x] Track kills, deaths, and time played per user.
  - [x] Track room totals for kills, deaths, and time played (time played = sum of all players).
- [~] Paid ability gating (e.g., pay to drop an obstacle at a chosen location) using x402 or on-chain proofs with host validation.

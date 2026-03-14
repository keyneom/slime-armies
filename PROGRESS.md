# Slime Armies MMO - Progress Tracker

## Original Objective
Transform "One Slime Army" from a single-player WASM-4 game into a P2P multiplayer game with:
- Faithful recreation of the original game's visual style
- P2P multiplayer using WebRTC (Matchbox)
- Infinite procedurally generated world
- Wave-based enemy spawning scaled by player count

## Current Session - Enemy Sync Fix

### Open Issues (Priority)
- [ ] Multiplayer sync smoothness/regression: remote players and enemies can still appear jumpy/laggy under real device-to-device conditions.
- [x] Title UI duplication: `SOLO PLAY`/`CREATE ROOM` were being rendered twice on start screen (low-res scene + overlay). Fixed by moving title text/UI rendering to overlay only.
- [x] Root-cause freeze fix for post-detach sessions: `vendor/matchbox_socket` message loop no longer spins on a closed signaling event stream (`events_receiver.next() -> None`), which was starving gameplay data-channel processing after signaling detach.

## Current Session - Multi-Supernode Relay Tree (In Progress)

### Goal
Reduce centralized relay load and move from single-supernode fanout to a cascading multi-supernode tree with area-aware routing.

### Implemented this session
- [x] Added `NETWORK_SCALING_PLAN.md` with research + phased architecture.
- [x] Added control-plane protocol messages:
  - `TopologyUpdateEvent`
  - `AreaAuthorityUpdateEvent`
- [x] Added `area_id` metadata to hot-path batch payloads:
  - `PlayerStateEntry.area_id`
  - `InputFrameEntry.area_id`
- [x] Added dynamic relay topology state in `NetworkSession`:
  - `supernode_set`, `super_root_id`, `relay_parent`, `relay_backup_parent`, `relay_children`, `relay_epoch`
- [x] Added dynamic area authority map (`area_id -> authority_hash`) with periodic root broadcast.
- [x] Changed state/input routing to parent/children cascade:
  - Upstream: leaf/child to parent
  - Downstream: parent to children (filtered by area + distance)
- [x] Changed enemy sync + wave start fanout from “root to all peers” to “root to children” with child forwarding.
- [x] Added runtime backup-parent failover:
  - Track per-peer last-seen frames
  - Switch to backup parent after timeout + cooldown
  - Route via `active_parent` for upstream traffic
- [x] Root now sends per-peer topology assignments (`parent`, `backup_parent`, `children`) instead of generic hints.
- [x] Dynamic supernode count now depends on both room size and active world-area spread.
- [x] Dynamic relay fanout now scales with room size.
- [x] Sticky anchor switching now uses hysteresis margin + distance-based forced reassignment.
- [x] Parent handoff now uses a short duplex upstream window for smoother transitions.
- [x] Forked `matchbox_socket` locally (server-protocol compatible) with selective desired-peer connectivity.
- [x] Network session now feeds desired links (parent/backup/children/supernode+witness/bootstrap) to socket.
- [x] Bootstrap now starts with normal signaling connectivity, then converges to sparse desired links after peer discovery/topology assignment.
- [x] Added explicit signaling detach control in socket fork (discovery can end while gameplay data channels stay up).
- [x] `PeerLeft` no longer forcibly closes active data channels (supports discovery-leave + persistent gameplay overlay).
- [x] Added session-level two-layer behavior:
  - regular nodes keep discovery attached during bootstrap only;
  - after overlay parent/route stabilizes, they detach from signaling;
  - detached nodes keep using relayed topology instead of local re-election.
- [x] Added optional TURN fallback configuration (runtime ICE override + TURN injection via `window.slime` APIs).
- [x] Added bootstrap authority fallback for direct peers during early convergence (prevents early PlayerUpdate/EnemySync drops before topology routes settle).
- [x] Stale-supernode detection now ignores pre-traffic bootstrap window to avoid premature supernode poisoning.
- [~] Extended cascaded routing to additional event classes:
  - kill/death, paid obstacle/ability, paid acks, chat, vote-mute, cannon shots now use tree relay paths.
  - late-join state sync still uses direct host sends (kept for now).
- [~] Added runtime relay telemetry + guardrails:
  - counters for recv/sent(upstream/downstream/broadcast), drops, queue depth, stale-parent switches.
  - capped relay/downlink queue sizes and capped batch payload sizes to prevent runaway backlog.
  - adaptive state/input send cadence under congestion (`relay_congestion_level`) to reduce message pressure.
  - in-game net debug overlay (`F3`) to view relay pressure/traffic/drop metrics live.
  - periodic telemetry console summary for larger rooms (throttled).

### Remaining work in this track
- [~] Tune congestion thresholds and per-topic budgets via larger room load tests.

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
- [x] Smooth spawn pacing: movement-based budget + jittered cadence (no bursty wave dumps)
- [x] WaveStart now seeds RNG properly (deterministic seed shared to clients)
- [x] Added Wisp enemy type with network sync and rendering
- [x] Added paid abilities: Bubble Shield and Shockwave (network gated + supernode ack)
- [x] Added Shrinefinder badge + shrine encounter at (67, 67)
- [x] p2pago SDK bundled (UMD) + support gating hooks for paid abilities
- [x] Added dedicated browser automation entry point (`window.slimeTest`) with input injection + runtime snapshot helpers for test harnesses (no gameplay/UI behavior changes for normal users)
- [x] Added buffered automation log access via `window.slimeTest.logs()` for sync triage
- [x] Added regression coverage for discovery-rooted topology election to prevent split-room self-election during overlay convergence

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

### Sync Testing (Automation-Friendly)
1. Prefer separate browser windows for each client under test; tabs are more likely to be background-throttled.
2. Create room in window A and wait until `window.slimeTest.net()` shows a real `local_peer=PeerId(...)`.
3. In each additional window: set `localStorage.setItem("slime_room_code", "<ROOM>")`, reload, then call `window.slimeTest.joinCurrentRoom()`.
4. Start movement keepalive in all windows (`window.slimeTest.keepAliveStart(360, 0.72)`) so slimes stay alive.
5. Poll each window with `window.slimeTest.net()` + `window.slimeTest.state()` + `window.slimeTest.logs()` every 1s.
6. Regression signature: one window keeps a silent/stale connected peer while other windows form a separate subgraph, or a window logs `Discovery detached: using gameplay overlay links only` then drops/freeze behavior and falls back toward solo (`remote_players=0`).

## Requested Features
- [x] Team scoring system that accounts for kills, deaths, and time played.
  - [x] Track kills, deaths, and time played per user.
  - [x] Track room totals for kills, deaths, and time played (time played = sum of all players).
- [~] Paid ability gating (e.g., pay to drop an obstacle at a chosen location) using x402 or on-chain proofs with host validation.
  - [x] Paid ability event + supernode ack flow (Bubble Shield, Shockwave)
  - [ ] On-chain or external receipt verification
- [~] Player naming policy:
  - [x] Room-level unique display names (deterministic suffixing for duplicates)
  - [x] Paid in-room name reservation command (`/buyname [NAME]`) via existing payment-gated feature flow + supernode/2-of-N verification
  - [x] Title-screen reserved-name guard: block create/join and warn if cached reservation belongs to a different local owner identity
  - [ ] Optional paid global name reservation/ownership flow (ENS or contract-backed), without requiring payment for unreserved names
- [ ] Define and deploy a generic immutable smart contract to verify paid unlocks and distribute tournament winnings; winner determination likely needs a separate consensus process.
- [ ] Future upgrades (laser attack, stronger shields) and competitive crypto prizes (last-man-standing, PvP tournaments).
- [ ] Mobile-friendly UX (touch controls, responsive HUD, safe areas for chat/map/player list).

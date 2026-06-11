# Slime Armies MMO - Progress Tracker

## Original Objective
Transform "One Slime Army" from a single-player WASM-4 game into a P2P multiplayer game with:
- Faithful recreation of the original game's visual style
- P2P multiplayer using WebRTC (Matchbox)
- Infinite procedurally generated world
- Wave-based enemy spawning scaled by player count

## Current Session - Topology Control-Plane Rewrite (sticky root + map)

### Why
Nodes were dropping in and out constantly and rooms split into subgraphs where
players could not see each other. Root causes found (see
`NETWORK_SCALING_PLAN.md` for the full write-up):
1. Per-frame, per-node root election over asymmetric membership views
   (matchbox only announces peers that join after you) -> split brains.
2. Tree assignments derived from player positions/RTT/load -> normal movement
   reshuffled parents/children room-wide, bumping epochs continuously.
3. Epoch-rotating witness/admission links + force-drop of any connected peer
   not in the latest desired set (in BOTH session and socket fork) -> a
   self-sustaining connect/disconnect churn loop.
4. Per-peer `TopologyUpdate` had no addressee; forwarded copies were applied
   by grandchildren as their own assignment -> poisoned routes at depth >= 2.
5. Discovery detach had no re-attach path; a detached node whose parent died
   could never form new links -> local re-election -> split rooms.
6. `is_host` followed lowest-uuid election, so authority flapped on joins.

### What changed
- [x] `TopologyUpdate` is now a room-wide map: epoch, root, fanout, and
      `{peer_hash, uuid, parent_hash}` per member. Identical for everyone,
      forwarded verbatim down the tree; each node derives its own links.
- [x] New `JoinRequest` message (tag 30) relayed up to the root; the root
      admits members with sticky BFS-by-join-order parent assignment and
      prunes silent members (~10s TTL, refreshed by relayed batch traffic).
- [x] Room creator is the sticky root/host. No per-node election. Root death
      is handled by staggered successor takeover from the shared map; rival
      roots converge via (epoch, root-hash) ordering + abdication.
- [x] Link policy is make-before-break: desired peers gate outgoing offers
      only, incoming offers are always accepted, undesired-but-healthy links
      get a ~10s grace before lazy drop, and the socket fork no longer closes
      channels on desired-set changes or mid-handshake desired drift.
- [x] Stable admission links (root/supernodes -> newcomers) and rescue links
      (root -> silent members) guarantee one side of any recovery pair can
      always initiate, regardless of signaling join order.
- [x] All nodes stay attached to signaling (membership oracle + only way to
      mint links). Discovery-detach removed from the session (fork API kept
      for a future overlay-relayed-signaling mode).
- [x] Direct `PlayerUpdate`s are accepted as the sender's own state claim
      (fixes "connected but blind" during convergence).
- [x] Player-state batches periodically bypass interest filtering (every 8th
      flush in rooms <= 128, every 32nd beyond) so distant players stay on
      rosters/minimaps at low rate.
- [x] `mark_supernode_bad` no longer poisons the root; it forces parent
      failover + a fresh JoinRequest.
- [x] The root is a replaceable coordinator role, not a fixed node: signaling
      departure of the root arms a fast failover (successor in ~3s, staggered
      by uuid rank); silent death falls back to a ~30s timeout. Stale-map
      echoes between orphans no longer count as root liveness (same-epoch
      maps refresh liveness only when they arrive from the parent/root path).
- [x] Hidden-tab watchdog: rAF stops entirely for occluded/backgrounded
      pages, which froze the whole client (no heartbeats -> pruned from the
      room; a dead root could never be replaced). A 500ms interval now drives
      the tick when rAF stalls, and the network runs on a wall-clock frame
      counter so its timeouts/cadences stay correct at any tick rate.
      Backgrounded players now stay in the room instead of dropping.
- [x] Map heartbeat doubles as root liveness; cadence widens in big rooms.
- [x] Unit tests for: sticky admission, map adoption (no self-election),
      stale-map rejection, staggered root failover, host abdication ordering,
      epoch-stable desired links, undesired-link grace, wire roundtrips.

### Verification (2026-06-10)
- `cargo test` (24 tests) green; `cargo check --target wasm32-unknown-unknown`
  clean; `trunk build` succeeds.
- Browser smoke (`scripts/sync_smoke.mjs`, 3 windows, headless Chrome + local
  matchbox server as test fixture): **PASS twice consecutively** — create +
  join-anytime, full 3-way roster with correct names on every tab, movement
  replicated to all tabs. Run with:
  `SLIME_SIGNALING=ws://127.0.0.1:3536 node scripts/sync_smoke.mjs 3`
  (needs `trunk serve`, Chrome with `--remote-debugging-port=9222`, and
  `matchbox_server` running locally; omit `SLIME_SIGNALING` to use the public
  server).
- Creator-departure drill (the room must outlive the first node): with a
  converged 3-player room, force-closing the creator's tab makes the
  survivors detect the departure via the room-wide signaling broadcast and
  promote a successor coordinator in ~3-5s (uuid-ordered, staggered); a
  transient dual-promotion under extreme tab throttling converges via the
  (epoch, root-hash) outranking + abdication rules. End state observed: one
  host, both survivors mutually visible with correct names, stable epoch.
- Baseline comparison: the same smoke run against a `HEAD` worktree (with only
  the ICE-gathering cap backported so it could form links in this sandbox at
  all) reproduced the reported disease in one run: the room creator (SYNCA)
  ended up alone with `discovery_attached=false` and `desired={}`,
  force-dropping both peers every frame, while SYNCB/SYNCC formed a separate
  2-player session in which SYNCB had elected itself host. The new build never
  exhibits this (sticky root, no detach, no force-drops).

### Scaling mechanics session (2026-06-10, later)
- [x] `TopologyDelta` (tag 31): delta-based roster broadcasts above 32
      members, empty-delta heartbeats, checksummed with full-map fallback on
      any desync, full-map anchors every 10th broadcast, coalesced dirty
      broadcasts. Unit-tested to 2,000 members (fanout/depth bounds, ~64KB
      full map vs <100B join delta).
- [x] Auto-rejoin: signaling/socket loss reconnects to the same room with
      backoff instead of erroring to title. connect() now fully resets relay
      topology state (stale routes used to poison the desired set after
      reconnect). Drill: 12s matchbox outage under a live 3-player room ->
      full recovery, positions preserved.
- [x] Connectivity-aware parent reassignment: repeated join-request nags
      from an in-roster member move it under a different parent (threshold,
      cooldown, subtree-safe) — the tree is the TURN-free relay fallback.
- [x] 29 unit tests green; smoke PASS; server-restart drill PASS;
      creator-kill drill PASS (chained after the server outage).

### Live-test fixes (2026-06-11, from real two-profile testing + review)
- [x] Frozen enemies: enemy-sync cadence used `frame % stride == 0` on the
      wall-clock net frame counter. Occluded/background tabs tick on the
      watchdog at exactly 500/1000ms, so the counter advances in fixed jumps
      (30/60) and the modulo locks onto a non-zero residue — enemy sync never
      fires again while player updates (counter-based) keep flowing. Cadence
      is now delta-based. Repro showed client spiders frozen bit-identical
      for 30s; post-fix they track the host in lockstep.
- [x] Sim catch-up: a hidden host used to simulate at ~1 frame/s (frozen
      world for everyone). The tick now runs up to 30 extra sim steps to
      track wall time, with edge inputs degraded to holds across the
      fast-forwarded steps. A fully occluded host now simulates ~30 fps
      effective.
- [x] Review P1: root member TTL is now refreshed from relayed batch entries
      (`note_member_seen` per entry), so depth >= 2 members aren't pruned
      while their state is actively flowing.
- [x] Review P1: handshake timeout raised 6s -> 15s (the wasm ICE-gathering
      cap can eat 3s on each side before the answer is even sent; 6s could
      deterministically timeout slow-ICE peers into a retry loop).
- [x] Review P2: area-authority updates are now accepted from the parent
      path (they're root-originated but tree-forwarded), so depth >= 2 nodes
      get real area maps instead of keeping stale/empty ones.
- [x] Added `test_enemy_snapshot()` probe (tick, last sync tick, counts,
      sample positions) for sync debugging.

### Smoothness session (2026-06-11, later)
- [x] Client-side enemy prediction: all clients run the enemy movement AI
      every local frame; host syncs are corrections (blend 35%, snap >240px
      or on revival). Replaces the interpolate-between-syncs model that left
      enemies frozen/stuttering whenever syncs slowed. Authoritative side
      effects (waves, shrine, cannon shots, guardian regen) stay host-only;
      clients discard predicted cannon-fire events.
- [x] Throttled-root handoff: a root that detects sustained background
      throttling (3+ watchdog-rate ticks) reassigns the root slot to another
      member via the normal map mechanism and abdicates; cascades until a
      foreground node holds it. Map-acquired hosts seed member liveness so
      the first root pass cannot mass-prune.
- [x] Fanout override (`test_set_fanout`, SLIME_FANOUT env in the smoke
      harness) forces deep topologies in small rooms; 3-tab chain smoke
      (root -> child -> grandchild) passes: names, movement, enemy sync all
      replicate through depth 2.
- [x] Verified live: baseline client enemy motion in 15/15 consecutive
      500ms samples; naturally occluded host hands off within seconds and
      the foreground player continues the world at full rate (31 unit tests
      green).
- [ ] Next (recommended): per-area enemy authority assigned to the nearest
      player, with root spot-checks — see NETWORK_SCALING_PLAN.md.

### Also fixed along the way (pre-existing, exposed by the smoke run)
- Handshakes stalled forever when STUN was unreachable: the vendored socket
  waited for ICE gathering to complete before sending any offer/answer, while
  the 6s handshake timeout kept firing -> infinite retry loop, zero
  connections. Gathering wait is now capped at 3s; trickle ICE delivers late
  candidates. This also un-breaks players behind UDP-blocking firewalls.
- Input pipeline applied all key-downs before all key-ups within a frame, so
  a same-frame "release+press" (the automation helpers, fast taps) silently
  ate held keys: `moveFor`/`keepAliveStart` never moved the player. The input
  buffer is now a single ordered event list.
- `PlayerJoined` handling migrated the *relayer's* peer state onto the
  *origin's* key for relayed joins, corrupting the relayer's roster entry
  (players showing as "PLAYER"). Alias migration now only happens when the
  direct sender is the origin. Control events (names/chat/votes) now flood to
  sibling subtrees, and each node re-announces its name every ~10s.

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

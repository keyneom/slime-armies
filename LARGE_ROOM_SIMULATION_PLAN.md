# Large-Room Refactor and Local Simulation Plan

Status: implementation handoff. This is the active build plan for large rooms;
`NETWORK_SCALING_PLAN.md` remains useful background on the current relay tree.

## Outcome

Make room cost depend on what a player can see, not total room population.
Keep Matchbox as the persistent membership/signaling control plane and WebRTC
as gameplay transport. Add a native, deterministic room simulator that runs
the same protocol, routing, queueing, and game-simulation rules as production
without launching browsers.

This is scalable co-op infrastructure, not cheat-proof competitive authority.
A client-owned cell can be made resilient and auditable, but a malicious
client still requires a trusted service or independently verifiable simulation
to be fully authoritative.

## Target architecture

| Layer     | Build                                                                                                                    | Why                                                                             |
| --------- | ------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------- |
| World     | `CellId { x, y }`, `InterestSet`, cell replicas, and cell-scoped entities                                                | A client simulates and receives only its current cell plus a fixed halo.        |
| Overlay   | The existing tree carries control; each active cell has a bounded multicast route through its authority and local relays | Socket count stays bounded without making the global root relay hot cell state. |
| Authority | A root-issued, expiring `CellLease` assigns one live subscriber to simulate each active cell                             | Enemy simulation moves off the global host without authority flapping.          |
| Transport | Reliable ordered control/snapshots; unreliable sequenced state with byte caps and latest-state coalescing                | State loss does not block membership, leases, or snapshots.                     |

Use the existing four-by-four chunk area as the initial cell size, but replace
the packed `u32` area id with a lossless signed `(x, y)` wire type. Do not use
the current global `u16` enemy ids/counts for the new protocol. Give an entity
a cell, generation, and local id; snapshots are byte-capped and fragmented.

### Required invariants

- Gameplay ticks must not scan the full `remote_players` map or load chunks
  around every room member. A peer holds only its `InterestSet` and coarse
  presence/minimap summaries for distant players.
- A cell authority simulates only its leased cells. A `WavePlan` carries a
  wave id, seed, and per-cell spawn budget; it replaces global wave/enemy
  snapshots.
- State messages carry `{cell, lease_epoch, tick, sequence}`. Older state is
  discarded; a gap requests a bounded snapshot rather than replaying history.
- Every outbound lane has a byte limit. State coalesces by entity; control may
  retry but may not grow without bound.
- V2 rooms must not mix old global-state messages with cell messages. Put the
  protocol version in join/capability negotiation and fail closed on mismatch.

## Build order

### 1. Separate policy from browser transport, then establish a baseline

Extract the topology, relay, subscription, and queue policy from
`src/net/session.rs` into a native-only `OverlayCore`. It takes peer/link/byte
events and emits `{peer, lane, bytes}` actions. Keep `NetworkSession` as the
WebRTC/Matchbox adapter. Add a `SimTransport` implementing the same boundary.
Use `src/net/{core,transport}.rs` for that seam, `src/simulation/` for
headless world/client state, and `src/sim/{scenario,transport,room,metrics}.rs`
plus `src/bin/room_sim.rs` for the harness.

Add `src/sim/` and a native `room-sim` binary. It must use real
`NetMessage::to_bytes`/`from_bytes` and `OverlayCore`, not a second fake
router. Extract the non-render multiplayer work in `src/lib.rs` into a
reusable `ClientLoop::step`, and pull headless world simulation out of
`Game`. The browser calls that loop; the simulator does too. Move clock and
randomness behind injected, seeded inputs so a failing run is exactly
reproducible from its seed and scenario name.

Done when existing topology/session tests run through the core and the
simulator can reproduce a join, relay, reconnect, and root-handoff scenario.

### 2. Make the game cell-local

Add `CellId`, `InterestSet`, `CellReplica`, `CellLease`, and `WavePlan` to the
network/world boundary. Replace the global remote-player input to
`Game::update_game_multiplayer` with nearby player replicas. Update
`ChunkManager` from "all player positions" to local interest plus cells the
local peer currently owns.

Implement subscription propagation: a leaf announces interest changes upward;
each relay maintains the union needed by each child subtree. For each active
cell, select a bounded multicast route rooted at its authority, with local
relays where needed. The control tree distributes the route, but high-rate
cell deltas must not first pass through the global root. Keep a low-rate coarse
presence feed for the minimap, not full transforms. Use the existing
desired-peer mechanism to make these links and cap the extra cell links; never
form a cell-wide mesh.

Done when adding distant players does not change a stationary peer's loaded
chunks, target list, enemy count, state bytes, or memory beyond the compact
presence directory.

### 3. Add leases, deltas, and real traffic lanes

The root issues versioned cell leases to an eligible subscriber and reassigns
only on expiry, departure, or a hysteresis threshold. The old area-authority
map can seed this work, but it is not the subscription system. Authorities
send cell deltas at the state cadence and a fragmented reliable snapshot on
join/resync; clients predict only subscribed cells.

Expose two WebRTC data channels in the Matchbox fork: reliable ordered
`Control` and unordered/unreliable `State`. Measure buffered bytes per peer;
apply a shared cap before enqueueing, coalesce old state, and report drops.
Control includes topology, subscriptions, leases, wave plans, and snapshots.

Done when loss or a slow peer delays state only; it cannot delay a lease,
membership update, or snapshot. A cell handoff has one monotonic lease epoch
and no double-authoritative stream.

### 4. Bind room control to an identity and roll out safely

Introduce `RoomGenesis`: a root public key bound to the room by a signed
invite or Matchbox-issued creator ticket. Sign topology maps, cell leases, and
wave plans; verify issuer, epoch, and sequence before applying them. Do not
treat a peer hash, relay hop, or two relay forwarders as proof of authority.

Ship cell replication behind a V2 room capability. Keep the current path only
for V1 rooms, never for mixed rooms. Matchbox remains connected for membership
and link repair. STUN discovers direct paths; deploy TURN for NAT failures --
the tree only helps after a player has formed at least one link.

## Local room simulator

Command target: `cargo run --release --bin room-sim -- --scenario <name> --seed <n>`.
It advances a virtual 60 Hz clock and should provide two modes:

- **Overlay mode:** thousands of lightweight peers running the actual codec,
  overlay, lane queues, topology, subscriptions, joins, and failures.
- **World mode:** active cell authorities plus client replicas running the
  extracted headless gameplay simulation; no renderer, DOM, or WebRTC needed.

Model per-link latency, jitter, bandwidth, loss, reordering, temporary link
failure, and a slow receiver. Model WebRTC semantics rather than raw UDP:
control is reliable/FIFO; state is lossy/sequenced; both obey the same byte
budget and coalescing rule as the browser adapter. Model Matchbox membership
separately from data links; desired links appear only after configurable
handshake delay or failure.

Start with these named, deterministic scenarios:

- `dense-1k`: 1,000 peers in 25 active cells; validates hotspot fan-out.
- `spread-1k`: 1,000 peers across 400 cells; validates AOI and cell leases.
- `migration-250`: groups repeatedly cross cell borders; validates handoff.
- `churn-500`: joins, leaves, root loss, and authority handoffs under delay.
- `slow-link`: 2-5% loss, jitter, and a congested relay/child; proves control
  survives state pressure.

Each run emits JSON with the seed, per-peer/cell bytes per second, queue high
water marks, drops/coalesces, control latency, state age, loaded cells,
authority handoffs, convergence error, CPU/work counters, and deterministic
checkpoint hashes. Assert:

- identical seed + scenario produces identical result;
- one accepted root/lease owner per cell and no unbounded queue;
- subscribed replicas converge after a stable network interval;
- adding distant peers does not increase a leaf's hot-state bytes or simulated
  entities; and
- a failure recovers through the normal control path without a split room.

Run a short 100-peer deterministic case in CI. Run `dense-1k`/`spread-1k` and
a 10,000-peer overlay-only benchmark locally in release mode. Start with the
existing recovery targets: clean root loss within 8 seconds and silent root
loss within 35 seconds under the scenario's configured link conditions.

The simulator is production-equivalent for protocol, routing, scheduling, and
backpressure. It cannot prove browser ICE, real NAT behavior, or TURN quality.
Keep the existing multi-window smoke test and add a small staging TURN drill
before calling a release production-ready.

## Do not optimize around the wrong constraint

Do not raise tree fanout, packet-count caps, or host snapshot frequency as a
substitute for AOI. The current global `remote_players`, global enemy targeting,
global chunk loading, and full `EnemySync` are the scaling boundary; solve
those before pursuing deeper trees or overlay-relayed signaling.

-------------------------------------
For Comparison and Consideration
-------------------------------------

Recommend a new concise `MASSIVE_ROOM_IMPLEMENTATION_PLAN.md`; the existing scaling plan is partly historical.

```md
# Massive Room Implementation Plan

## Goal

Support large backendless rooms without making every client simulate, store, or
receive the whole world. Matchbox remains signaling/membership; WebRTC carries
gameplay.

## Rules

- Global topology is control-plane only; it must not carry high-rate cell state.
- Clients simulate and receive only nearby cells plus a border halo.
- Every fast message has a byte budget, lane, sequence, and drop/coalesce policy.
- The local simulator uses real protocol/routing code, not mocks.
- Public-room control requires signed room genesis and leases.

## 1. Extract testable seams

Create render-free `WorldSim` plus a `Transport` boundary.

- Move deterministic world ticking, spawning, entity ownership, and replication
  inputs out of `Game`/the browser loop.
- `Game` becomes UI/input/render glue.
- `Transport` exposes peer events, `poll`, and `send(peer, lane, bytes)`.
- Keep production WebRTC and virtual transport behind that shared boundary.
- Record bytes, queue bytes, packet age, drops, authority changes, and cell count.

Done: native tests can tick a room without browser APIs.

## 2. Build the deterministic local room simulator

Add native `room_sim`.

- Scenarios specify seed, population/distribution, movement, joins/leaves, NAT
  connectivity, latency/jitter/loss/reorder, bandwidth, slow peers, and partitions.
- Virtual links enforce the intended two lanes and backpressure. Packets travel
  hop-by-hop as encoded bytes and decode through production handlers.
- Run dense-room, spread-world, migration, root-loss, hard-NAT, and slow-peer
  scenarios. Emit JSON metrics and invariant failures.
- Keep browser smoke/jitter scripts as small real-WebRTC calibration tests.

Done: seeded scenarios reproduce results and report bounded queue/memory, p95 state
age, bytes per peer, links, and visible-state divergence.

## 3. Add cell AOI and routing

Replace room-wide state broadcast with cell subscriptions.

- Add `CellId`, `InterestSet`, `CellLease`, and aggregated child subscriptions.
  Players subscribe to their current cell plus neighbor halo; update only on change.
- A lease names authority, epoch, relay root, expiry, and signature.
- The global root coordinates leases but never relays cell deltas.
- Build bounded per-cell multicast trees. Relays forward only into subscribed
  child subtrees and migrate overloaded relay roles.

Done: a cell update cannot reach uninterested clients or pass through the global root.

## 4. Make simulation and replication cell-local

- Replace global enemy/player scans with a cell spatial index.
- Simulate owned cells and halo only; load chunks only for those cells.
- Change waves from global enemy count × room players to a global schedule/seed
  plus per-active-cell spawn budgets based on local population.
- Replace `u16` enemy ids with `(CellId, generation, local id)` or equivalent.
- Use reliable fragmented `CellSnapshot` for subscribe/handoff and sequenced
  `CellDelta` for steady state. Keep critical outcomes reliable/idempotent.
- Handoff: snapshot successor, advance lease epoch, then accept new lease only.

Done: repeated border crossings and late joins produce no duplicate/ghost entities.

## 5. Production transport and trust

- Use reliable ordered control/snapshots plus unordered unreliable-sequenced state.
- Replace unbounded socket queues with bounded byte queues. Coalesce newest state
  by `(cell, entity)`; surface backpressure.
- Gate sends on WebRTC buffered amount; recover overloaded links.
- Have the self-hosted Matchbox service issue signed room genesis and bind joining
  public keys to signaling identities. Sign topology, leases, and global events.
- STUN discovers direct paths. Configure TURN as a paid/self-hosted fallback for
  hard NATs; it relays only when direct P2P cannot form.

Done: forged control is rejected and slow peers cannot grow memory or stall state.

## Required scenarios

1. Dense cell.
2. Spread world with many active cells.
3. Repeated player/entity migration.
4. Authority/relay loss, partition, reconnect, slow tab.
5. Direct-path failures with and without TURN.
6. Forged origin, topology, and stale lease attempts.
```

Concrete ownership seams:

- `src/game.rs`: global remote-target scans, chunk loading, and total-player wave scaling.
- `src/lib.rs`: browser loop and current global enemy-sync orchestration.
- `src/net/session.rs`: split global control from cell subscription/routing.
- `src/net/protocol.rs`: add cell/lease/snapshot/delta messages; retire global `EnemySync` at scale.
- `vendor/matchbox_socket/src/webrtc_socket/{socket.rs,mod.rs,wasm.rs}`: two channels exist already, but queues are unbounded and lack buffered-amount handling.
- Add `src/bin/room_sim.rs` and `src/sim/{room,transport,scenario,metrics}.rs`.
- `matchbox-server/src/main.rs`: only for signed genesis/identity attestations.

Key warning: leaf-only filtering on the current tree is insufficient; it still pushes every active cell through root/interior relays. Cell multicast must be a distinct high-rate data plane.
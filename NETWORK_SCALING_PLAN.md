# P2P Scaling Research + Execution Plan

Last updated: 2026-03-06

## Why this exists
Current networking has one elected supernode and a batch relay path. That reduced message volume, but it still centralizes relaying and authority too much for true massive rooms.

This document defines a multi-supernode cascading relay design with dynamic area authority and a concrete implementation sequence for this codebase.

## Research Notes (applied, game-focused)

### 1) Tree fanout over full mesh
- Large-scale systems avoid full mesh broadcast.
- Practical pattern: bounded fanout trees where each node forwards to a small subset (`k` children).
- Benefit: propagation cost trends toward `O(N log_k N)` hops/messages, not `O(N^2)` peer fanout.

### 2) Area/interest partitioning
- Real-time worlds scale by locality.
- Most hot updates are relevant only to nearby players.
- Benefit: reduce per-node inbound load by topic + area (chunk neighborhoods), while keeping global consistency events sparse.

### 3) Multi-authority instead of single host
- One root should not be authoritative for all fast-changing data.
- Use topic-specific authority:
- Area authority for local world state and local relays.
- Root/super-root for membership, conflict tie-breaks, and epoch transitions.

### 4) Two-stage confirmation for critical events
- Keep optimistic application for responsiveness.
- Finalize on confirmation from two distinct sources (`2-of-N`).
- In this game that can be `origin + authority relay`.

## Target Architecture

## Overlay Roles
- `super_root`: deterministic top node (epoch leader) for room-wide control-plane decisions.
- `super_nodes`: elected relays with bounded children.
- `area_authorities`: elected per area (chunk group), ideally one of the local supernodes near that area.
- `leaf peers`: regular players connected to one parent, optional backup parent.

## Topology
- Directed relay tree with bounded fanout (`MAX_CHILDREN`).
- Parents forward downward; leaves send upward.
- Side links are optional and only for backup/failover, not routine broadcast.

## Discovery vs Gameplay Layers (No Matchbox Room Sharding)
- `Layer 1 (discovery/control)`: Matchbox signaling room.
  - Supernodes/root stay attached longer to keep join-path continuity.
  - Regular nodes attach temporarily for bootstrap.
- `Layer 2 (gameplay)`: WebRTC data-channel overlay (tree + bounded optional links).
  - After bootstrap, regular nodes detach from signaling and keep gameplay channels.
  - Relay/control messages continue over the gameplay overlay.
  - This avoids global signaling fanout to every regular node on each join/leave.

## Authority model
- Player action authority remains with originating player for direct action claims.
- Area authority verifies/aggregates area-scoped high-rate data.
- Super-root resolves conflicts and issues epoch/route updates.

## Data classes
- `hot area state`: player state, inputs, local transient events.
  - Route: `leaf -> parent -> area_authority -> area subtree`.
- `critical events`: kill/death/paid unlocks.
  - Route: `origin -> parent -> authority`.
  - Relay includes origin attestation; optimistic apply allowed; finalize on 2-of-N.
- `control plane`: membership/topology/epoch/authority map.
  - Route from super-root to all through tree.

## Routing keys
- `area_id`: derived from chunk coordinate (coarse grouping).
- `topic`: state/input/event/control.
- `epoch`: topology version for failover safety.

## Dynamic behavior
- Periodic re-election with hysteresis to prevent flapping.
- Parent switch only after minimum sample count and improvement threshold.
- Immediate failover to backup parent on timeout.
- Dynamic supernode count from both room size and world spread (active areas).
- Dynamic relay fanout from room size (low fanout in tiny rooms, higher in large rooms).
- Sticky anchor assignment for leaves:
  - Prefer current supernode unless a candidate is decisively better.
  - Force switch if current anchor becomes too far from player position.
- Short handoff duplex window on parent change:
  - During handoff, upstream messages go to active and primary parent.
  - Keeps continuity while topology converges after movement/teleport.

## Room-size behavior (current policy)

Baseline below assumes one dense hotspot (`active_areas = 1`). If players spread across more areas, supernode count increases.

| Players | Supernodes (baseline) | Fanout | Notes |
|---|---:|---:|---|
| 1 | 1 | 2 | Single-node local authority, no relay parent. |
| 2 | 1 | 2 | One root + one child. |
| 3 | 1 | 2 | One root, deterministic tie-break by peer id. |
| 7 | 2 | 3 | First split into two local relay heads. |
| 12 | 3 | 4 | Small multi-supernode room. |
| 21 | 4 | 5 | Stable clustered anchors per region. |
| 32 | 6 | 5 | Higher locality partitioning. |
| 100 | 6 | 7 | Dynamic area growth can raise this further. |
| 1,000 | 42 | 9 | Regionized relay strongly preferred. |
| 10,000 | 256 (cap) | 12 | Capped by `MAX_SUPERNODES`. |
| 100,000 | 256 (cap) | 12 | Requires deeper overlay/specialized transport. |

## Switch policy details

- Inputs used for node assignment/switch:
  - In-game proximity (distance to candidate supernode).
  - Real-world latency samples (`RTT`).
  - Current supernode load.
  - Previous anchor stickiness (hysteresis margin).
- Teleport behavior:
  - Large position jumps naturally exceed anchor distance threshold and force reassignment.
  - Parent handoff keeps previous path alive briefly while new parent is activated.
- Normal movement behavior:
  - Leaves stay on current anchor until improvement is meaningful, reducing oscillation near borders.
  - Backup parent remains available for stale/failed primary.

## Execution Plan

## Phase A (now): control-plane scaffolding
- Add topology/area metadata message types.
- Add relay header with `origin_hash`, `event_id`, `area_id`, `epoch`, `hops`.
- Keep existing behavior as fallback.

## Phase B: multi-supernode election + tree build
- Elect top `K` supernodes from latency score and stability.
- Build deterministic parent/children assignment with bounded fanout.
- Introduce backup parent.

## Phase C: area authority map
- Partition world into area cells (grouped chunks).
- Select authority per area from elected supernodes, preferring low-latency and local proximity.
- Broadcast map to peers.

## Phase D: route by tree + area
- Replace single supernode relay path:
- Upstream to parent only.
- Downstream only to children.
- Route by area/topic and perform interest filtering before forward.

## Phase E: 2-of-N finalize with optimistic apply
- Already optimistic for kill/death.
- Finalize with authority relay + origin attestation.
- Add rollback marker path for failed confirmations.

## Phase F: metrics + guardrails
- Track per-node send/recv counts, relay queue depth, drop rate, and stale parent events.
- Auto-throttle update rates when congestion is detected.

## Implementation Notes for this repo
- Primary code location: `src/net/session.rs`, `src/net/protocol.rs`, `src/lib.rs`.
- Keep deterministic fallbacks for tiny rooms (`<=2` players).
- Keep compatibility path until all peers support new topology messages.

## Current status checklist
- [x] Single supernode relay batching exists.
- [x] Interest filtering exists (single-root path).
- [x] Optimistic + confirmation path for kill/death exists.
- [x] Multi-supernode election (`K > 1`) implemented.
- [x] Tree parent/children routing implemented.
- [x] Area authority map implemented (dynamic from observed areas + supernode proximity/latency score).
- [x] Routed forwarding (upstream/downstream) implemented for player state, input batches, enemy sync, and wave start.
- [x] Control-plane epochs and failover path implemented (active parent + backup parent failover with cooldown).
- [x] Root now emits per-peer topology assignments (parent/backup/children) over `TopologyUpdateEvent`.
- [x] Local fork of `matchbox_socket` adds selective desired-peer connectivity while keeping Matchbox signaling protocol compatibility.
- [x] Sparse-link mode now activates immediately on connect (avoids initial full-mesh handshake burst).
- [x] Added signaling detachment path for regular nodes after overlay bootstrap (no room sharding required).
- [x] Active data channels now survive signaling `PeerLeft` (supports two-layer behavior).
- [~] Cascaded routing expanded to control/event classes (kill/death, paid events+acks, chat/vote, cannon shots); late-join sync still direct.
- [~] Runtime telemetry + guardrails active (send/recv/drop counters + queue/batch caps + adaptive cadence + low-priority budgets); threshold tuning still pending.

## Landed in this iteration
- Added protocol messages: `TopologyUpdateEvent`, `AreaAuthorityUpdateEvent`.
- Added area metadata on hot-path batches (`PlayerStateEntry.area_id`, `InputFrameEntry.area_id`).
- Added dynamic relay topology state in session:
- `supernode_set`, `super_root_id`, `relay_parent`, `relay_backup_parent`, `relay_children`, `relay_epoch`.
- Replaced single-root batch fanout with cascading relay behavior:
- upstream to parent, downstream to children with area+distance filtering.
- Added dynamic area authority recomputation and broadcast from root.
- Added parent activity tracking + runtime failover to backup parent for stale links.
- Added per-peer topology updates broadcast from root (not just set/epoch hints).
- Added relay telemetry counters and queue/batch guardrails to prevent runaway relay backlog.
- Added adaptive state/input send cadence under high relay queue pressure.
- Added in-game network debug overlay (`F3`) for live relay metrics.
- Added throttled periodic telemetry logging for larger rooms.

# P2P Scaling Research + Execution Plan

Last updated: 2026-06-09

## 2026-06-09 redesign: sticky-root topology map (CURRENT ARCHITECTURE)

The per-node election + per-peer topology assignment design below (kept for
history) proved structurally unstable: every node elected a root over a
different membership view (stock matchbox only tells you about peers that
joined *after* you), tree assignments depended on player positions/RTT (so
normal movement reshuffled routes), epoch bumps rotated witness/admission
links, and both the session and the socket fork force-closed any channel that
fell out of the desired set. The result was the observed constant peer churn
and split rooms. The current implementation replaces the control plane:

- `Single sticky root`: the room creator is the topology authority. Nobody
  else ever self-elects while a root is alive. The root assigns each member a
  parent once (BFS by join order, capacity = dynamic fanout) and never
  reshuffles existing routes on join/movement/latency changes.
- `One room-wide TopologyUpdate map` (epoch, root, fanout, entries of
  `{peer_hash, uuid, parent_hash}`), broadcast by the root and forwarded
  verbatim down the tree. Identical for all recipients: each node derives its
  own parent/children/backup/supernode-set from it, so forwarding can never
  poison anyone's routes (the old per-peer updates were applied by
  grandchildren as their own). The uuids let any node resolve any member to a
  connectable transport id.
- `JoinRequest` messages relay up the tree; the root admits members, prunes
  members silent for ~10s (liveness piggybacks on relayed batch traffic), and
  re-homes orphaned subtrees without touching anything else.
- `Root failover`: the root is a replaceable coordinator role, never a fixed
  dependency. A clean departure is broadcast room-wide by the signaling
  server and triggers fast succession (~3s, uuid order from the shared map,
  staggered by rank); silent death (crash, network loss) falls back to a
  ~30s map-staleness timeout. Competing roots converge via a total order on
  (epoch, root hash) plus abdication + "reply with winning map". Same-epoch
  map echoes between orphans of a dead root do not count as liveness; only
  higher-epoch maps or copies arriving from the parent/root path do.
- `Link policy is make-before-break`: desired peers only gate *outgoing*
  offers; incoming offers are always accepted; healthy-but-undesired links
  survive a grace window (~10s) before being dropped; nothing force-drops on
  a desired-set delta. Admission (root/supernodes -> roster-less newcomers)
  and rescue (root -> silent members) links are stable, not epoch-rotated.
- `Everyone stays attached to signaling`. The matchbox room is the membership
  oracle and the only way to mint new WebRTC links. An idle WebSocket per
  client is cheap at the hundreds-to-low-thousands scale; gameplay data still
  rides the sparse supernode tree, so per-node WebRTC link counts stay
  bounded. The old "detach after bootstrap" path stranded nodes permanently
  (no re-attach existed) and was the main source of split rooms; the socket
  fork keeps `detach_signaling` for a future overlay-relayed-signaling mode.

## Dependency stance: Matchbox control plane, P2P gameplay — and STUN

Goal: the public Matchbox server is the only third-party control-plane
dependency; gameplay data is peer-to-peer. Where that stands:

- `Gameplay data`: already pure P2P, before and after this redesign. No server
  ever carries game traffic; the relay tree is made of direct WebRTC links
  between players.
- `Membership/links`: the matchbox room is the membership oracle and the only
  place new WebRTC links can be minted (browsers cannot exchange SDP without
  *some* rendezvous). Staying attached costs exactly ONE websocket per
  client — to the server, not per peer. Browser connection-limit math, since
  it keeps coming up:
  - Websockets: 1 per tab total (limits are ~200-255 per tab; irrelevant).
  - WebRTC: RTCPeerConnections are not websockets and not bound by HTTP
    connection limits (Chrome allows ~500 per tab). Our tree caps per-node
    links far below that regardless of room size: leaves <= 7, supernodes
    <= 24, root <= 28. A 1,000-player room never asks any single browser to
    hold more than ~28 connections; fanout 12 at depth 3 spans ~1,700 nodes.
  - The O(N) cost lives on the signaling SERVER (N idle websockets, a
    NewPeer broadcast per join) — trivial for a websocket service, but note
    the public match-0-13.helsing.studio is someone's free community server:
    fine for playtests and hundreds of players, but parking thousands of
    concurrent users on it is freeloading and a single point of failure we
    do not control. `set_signaling_server()` retargets any
    matchbox-protocol-compatible deployment if/when that day comes.
  True detach requires overlay-relayed signaling (peers forwarding SDP through
  the tree) — planned, not yet built.
- `STUN`: cannot be fully removed for internet play, and here is why. Two
  browsers behind different NATs can only connect if each learns the public
  address:port its NAT assigned — that information only exists outside the
  NAT, so *someone* outside must echo it back. That is all STUN is: a
  stateless one-shot "what is my address?" echo used during link setup; no
  game data ever touches it, any STUN server is interchangeable, and the
  browser offers no other API to learn the mapping (native apps can observe
  peers' source addresses themselves; browser WebRTC cannot).

  We do not run any STUN infrastructure and never have: the game has always
  piggybacked on free public STUN (Google's, since the first networking
  commit). The default list now carries two independent operators —
  `stun.l.google.com:19302` / `stun1.l.google.com:19302` (Google) and
  `stun.cloudflare.com:3478` (Cloudflare) — queried in parallel, so one
  provider disappearing costs nothing. More free ones exist (Twilio,
  stunprotocol.org, Mozilla) if these ever rot.

  Ideas evaluated for replacing STUN, and why they don't work:
  - "Learn our UDP address from Ethereum / a wallet RPC / any web service":
    impossible — those are TCP/HTTPS connections, so they can only observe
    the TCP flow's NAT mapping (the same thing matchbox sees). NAT mappings
    are per-socket; the UDP socket WebRTC opens has its own mapping that can
    only be observed by something that *receives a UDP packet from it*. Any
    service that does that is, definitionally, a STUN server.
  - "Fall back to TCP when UDP info is unavailable": browser-to-browser
    ICE-TCP across NATs effectively does not work (TCP NAT traversal is
    harder than UDP, and browsers only generate passive/relay TCP
    candidates). TCP fallback in WebRTC really means TURN-over-TCP, i.e. a
    relay server — the thing we refuse to depend on.
  - "Tunnel game data through the matchbox websocket": technically possible
    (the Signal message relays arbitrary payloads) but it turns the
    signaling server into a data relay — worst of all worlds, and an abuse
    of a free community service.
  - Pay-per-use x402/crypto ICE service: STUN itself is too cheap to meter
    (hence all the free ones); the idea is sound for `TURN`, where bandwidth
    costs real money. The runtime hooks for that already exist
    (`set_ice_servers`, `set_turn_fallback`) — a paid TURN endpoint can be
    plugged in at runtime later with zero code changes here.

  The infrastructure-free fallback for pairs that cannot connect even with
  STUN (two symmetric/"hard" NATs) is the relay tree itself: every node only
  needs a working link to *somebody*, and the root can learn from failed
  link attempts and assign parents by connectivity rather than capacity
  alone. Peer-relaying replaces TURN as long as each player can reach at
  least one other player. (Connectivity-aware parent reassignment is future
  work; capacity-based assignment is what ships today.)
  - `set_ice_servers("none", "", "")`: zero ICE servers — works on a LAN
    (mDNS/host candidates), used by tests; not viable across NATs.

## Scaling mechanics (implemented 2026-06-10)

- `Delta topology maps`: rooms above 32 members broadcast roster deltas
  (~40 bytes per join) instead of full maps, with an empty delta as the
  ~30-byte liveness heartbeat and a full-map anchor every 10th broadcast.
  Every delta carries a checksum of the resulting roster; any receiver that
  desyncs (missed epoch, divergent checksum) detects it immediately and
  requests a full map through the join machinery. Membership-change
  broadcasts are coalesced (0.5s) in big rooms; joiners waiting on a map
  still get theirs immediately. Verified by unit tests up to 2,000 members:
  bounded fanout, tree depth <= 5, ~64KB full map vs <100B join delta.
- `Auto-rejoin`: losing the signaling websocket (server restart, network
  blip) no longer dumps the player to the title screen or strands the node.
  The session reconnects to the same room with backoff under a fresh peer
  id; the join machinery re-admits everyone and old identities age out.
  Drill: matchbox server killed for 12s under a live 3-player room -> all
  clients rejoined, a new coordinator emerged, full roster re-formed with
  game positions preserved.
- `Connectivity-aware reassignment`: an in-roster member that keeps sending
  join requests is telling the root its assigned parent link never forms
  (e.g. two hard NATs). The root moves it under a different parent
  (threshold + cooldown), making the tree itself the relay fallback for
  unreachable pairs — no TURN dependency.
- `Hidden/backgrounded tabs`: a 500ms watchdog drives the game tick when
  rAF stalls and the network clock is wall-time based, so throttled tabs
  stay in the room (slow-motion locally, live on the network).

## Smoothness model (implemented 2026-06-11)

- `Client-side enemy prediction`: every client runs the same enemy movement
  AI locally each frame (motion is always smooth at the local frame rate);
  the host remains the single authority — its periodic syncs *correct* the
  prediction (small errors blend at 35%/sync, >240px or revivals snap), and
  only the host produces side effects (wave/shrine spawns, cannon shot
  events, guardian HP regen). Wave-start RNG reseeding keeps deterministic
  spawns intact even though clients consume RNG for placeholders.
- `Throttled-root handoff`: browsers stop rAF for occluded/backgrounded
  tabs; the watchdog keeps such a tab alive at ~1Hz (sim catch-up ~30fps
  effective), but that is still a poor world authority. A root that detects
  sustained throttling hands the role to another member through the normal
  map mechanism (epoch bump, successor's entry becomes parentless) and
  becomes a regular member; if the successor is also throttled it hands off
  again, cascading to a foreground node. Verified live: occluding the host
  window migrates the role to the foreground player within ~3s and enemies
  keep moving at full rate for everyone.

## Toward area authority (recommendation, not yet implemented)

The end-state the project always sketched: the player nearest an enemy
simulates it authoritatively (their prediction is already the best source),
with the root only arbitrating membership and area ownership. What exists
today: area ids on hot-path entries, an area->authority map broadcast by the
root, and now identical enemy AI running on every client. The remaining
steps, in safe order:

1. Partition authority by enemy, not by message: the root's area map assigns
   each area's enemies to the member closest to that area (it already has
   coarse positions). The assigned member includes only *its* areas' enemies
   in its EnemySync; the host stops syncing those areas.
2. Everyone else treats those syncs as corrections exactly as they treat the
   host's today (the prediction/correction split just landed makes this a
   routing change, not a simulation change).
3. Conflict and anti-cheat: keep kills/deaths on the existing 2-of-N
   confirmation path; the root spot-checks area authorities by re-simulating
   a sampled area for a few frames and reassigns an authority whose stream
   diverges wildly or whose kill rate is anomalous. Authority gives smooth
   *motion*; it must not be allowed to unilaterally decide *outcomes*.
4. Handoff hysteresis: area ownership follows the sticky-anchor rule (only
   reassign on decisive distance change) so authority does not flap at area
   borders.

Known limits / next steps:
- Verified in-browser at small scale plus 2,000-member unit-level tree
  tests; a real load test (hundreds of headless clients) needs a native
  session harness or a fleet, neither of which exists yet.
- Parking thousands of concurrent players on the public community matchbox
  server is freeloading and a single point of failure; self-host the same
  binary when rooms get big (`set_signaling_server` retargets at runtime).
- For tens of thousands: shard signaling and overlay-relayed signaling so
  leaves can finally detach from the room websocket (the original two-layer
  ideal); deferred because it only pays off at that scale and weakens the
  departure-broadcast failure detection the current design relies on.

---

## Historical plan (pre-2026-06-09, superseded above)

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

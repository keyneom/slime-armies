# Slime Armies

A P2P massively multiplayer online game inspired by "One Slime Army". Fight waves of enemies in an infinite procedurally generated world with other players.

Inspired by the original "One Slime Army" (WASM-4).

## Controls

- **WASD / Arrow Keys**: Move
- **Z / Space**: Attack (hold to block, release to swing)
- **X / Shift**: Dodge
- **R**: Bubble Shield (paid ability, default)
- **F**: Shockwave (paid ability, default)
- **T**: Slow spawn (paid ability, default)
- **G**: Speed boost (paid ability, default)
- **Y**: Slime trail (paid ability, default)
- **C**: Open chat (`/mute NAME`, `/buyname [NAME]`)
- **F3**: Toggle network debug overlay (relay queue/traffic stats)
- **M**: Open map (arrows pan, Z zoom in, X zoom out, type coords, Enter to teleport)

## Development

```bash
# Install dependencies
rustup target add wasm32-unknown-unknown
cargo install trunk

# Build for GitHub Pages (docs/)
./scripts/build-docs.sh

# Run dev server
./scripts/serve.sh

# Build for production
./scripts/build.sh
```

**Note:** Use the `scripts/` wrappers for trunk; they unset `NO_COLOR` / `FORCE_COLOR`, which otherwise can cause `invalid value '1' for '--no-color'` when running `trunk` directly (e.g. in Cursor's terminal or CI).

## TURN Fallback

You can configure ICE/TURN at runtime from the browser console:

```js
// Add TURN fallback while keeping default STUN:
window.slime.set_turn_fallback("turn:turn.example.com:3478?transport=udp", "user", "pass");

// Or fully override ICE URL list (comma-separated):
window.slime.set_ice_servers(
  "stun:stun.l.google.com:19302,turn:turn.example.com:3478?transport=udp",
  "user",
  "pass"
);

// Reset to default STUN-only config:
window.slime.reset_ice_servers();
```

## Test Automation Entry Point

For browser automation, the app exposes `window.slimeTest` (additive only; regular user controls are unchanged).

```js
// Key and text input (same internal input path as real events):
window.slimeTest.keyDown("KeyW", "w");
window.slimeTest.keyUp("KeyW");
window.slimeTest.typeText("HELLO");

// Canvas click in game coordinates:
window.slimeTest.click(400, 300);

// Reset queued synthetic inputs:
window.slimeTest.resetInput();

// Compact runtime snapshot:
window.slimeTest.state();
// "scene=game;network=connected;room=ABC123;players=2;map_open=false;player_list_open=false;chat_open=false"

// Focused network diagnostics:
window.slimeTest.net();
// "network=connected;room=ABC123;remote_players=1;known_peers=2;desired_peers=3;discovery_attached=true;relay_epoch=4;is_host=false;local_name=KEY#1;rx=123;dropped=0;remote_ids=...;remote_names=..."
```

### Sync Testing (Automation-Friendly)

Use this workflow to reproduce/validate multiplayer sync stability and freeze regressions.

1. Open separate browser windows for each client under test. Separate tabs can be background-throttled by the OS/browser and are less reliable for sync debugging.
2. In window A, create a room and wait until `window.slimeTest.net()` shows a real `local_peer=PeerId(...)`.
3. Read window A room code from `window.slimeTest.net()` (`room=XXXXXX`).
4. In window B console, set room + reload + join:
```js
localStorage.setItem("slime_room_code", "ROOMCODE");
location.reload();
// after reload:
window.slimeTest.joinCurrentRoom();
```
5. Repeat for any additional windows after the creator is already attached to signaling.
6. Immediately keep all slimes moving so they do not die during sync checks:
```js
window.slimeTest.keepAliveStart(360, 0.72);
```
7. Poll all windows every 1s:
```js
window.slimeTest.net();
window.slimeTest.state();
window.slimeTest.logs();
```
8. Healthy behavior: all windows agree on player count/names and continue receiving traffic (`rx` rises, `remote_players` matches expected peers).
9. Split-room regression signature: one window shows a connected peer with a growing stale age or rising silence warnings, while other windows form a separate subgraph and do not list the creator.
10. Freeze regression signature: one window stops receiving gameplay traffic or reports a growing stale/silence age, then becomes unresponsive or drops; the other windows eventually report fewer peers.

Notes:
- Use `window.slimeTest` for automation. `window.slime` exposes low-level wasm exports and is not safe for direct string-argument automation calls.
- `window.slimeTest.logs()` returns the buffered page console tail, which is the first thing to inspect when a peer stalls or splits.
- Keep this as the default sync regression test until a fuller automated harness lands.

## Architecture

- **Rust + WebAssembly**: Core game logic
- **Canvas 2D**: Rendering
- **Matchbox**: P2P networking
- **Socket fork (compatible)**: local `matchbox_socket` fork keeps Matchbox signaling protocol compatibility while enabling selective peer links
- **Two-layer P2P**: persistent Matchbox membership/signaling control plane + long-lived WebRTC gameplay overlay (tree relay)
- **Signaling membership**: all clients remain attached so the room can repair membership and mint new sparse WebRTC links; Matchbox never carries gameplay data
- **Rollback netcode**: lightweight input/state rollback (in progress)
- **Hybrid P2P topology**: dynamic multi-supernode relay tree with area-aware routing, adaptive fanout, and parent failover (in progress)

## Roadmap

- [x] Phase 1: Core single-player (player movement, combat)
- [x] Phase 2: Enemies (spider, cannon, snake boss)
- [x] Phase 3: Infinite world with chunk loading
- [x] Phase 4: Basic P2P multiplayer
- [~] Phase 5: Rollback netcode (input/state rollback in place; full GGRS-style model pending)
- [~] Phase 6: Supernode architecture for scale (multi-supernode relay + failover in place; tuning pending)
- [x] Phase 7: World-wide minimap
- [ ] Phase 8: Polish and optimization

## Deployment

Designed to deploy as a static site on GitHub Pages with no dedicated backend servers.

## Future Considerations

### Quantum Theme Exploration

The game could transition to a quantum physics theme, where players control quantum particles instead of slimes. Potential features:

- **Quantum Tunneling** (already implemented as "Phase/Dodge"): Pass through obstacles and enemies
- **Entanglement**: Link with another player - when one is damaged, both take reduced damage; when one heals, both heal
- **Superposition**: Player exists in multiple states simultaneously, confusing enemies or allowing split attacks
- **Wave-Particle Duality**: Toggle between wave form (spreads out, passes through gaps) and particle form (focused, high damage)
- **Quantum Sensing**: Detect hidden enemies or traps at a distance
- **Teleportation**: Instant movement to an entangled partner's location

Visual effects could draw inspiration from quantum wave functions collapsing into particles, probability clouds, and interference patterns.

### Paid Ability Gating (No Dedicated Backend)
We could offer paid abilities (example: drop an obstacle at a chosen coordinate) using a payment-gated unlock flow such as x402. In a fully static client, true enforcement is hard without an authoritative server or host validation; a modified client could bypass local checks. More secure options:
- **Host-authoritative validation**: the host verifies a signed on-chain receipt/token before broadcasting the ability to all peers.
- **On-chain capability NFT/token**: the ability is tied to a token; the host (or a lightweight validator) checks ownership on-chain.
- **Commit-reveal**: player commits to a placement, reveals after payment confirmation; host confirms before applying.
- **Username reservation (optional paid)**: names remain free by default; only globally reserved names require payment/proof of ownership.
If we keep this backendless, the best practical guardrail is host-side verification plus on-chain proofs (e.g., tx hash or signed receipt in the event payload), but it is not cheat-proof without an authoritative server.

We’ll need a generic, immutable smart contract that can verify paid unlocks for any feature (e.g., obstacle drops) without adding new methods per feature. It should also support prize payouts for tournaments and other competitive modes; winner determination likely needs its own consensus process, so contract hooks should expect externally agreed results.
Room-level uniqueness should always be enforced regardless of reservation status (duplicate handles get deterministic suffixes like `#2`, `#3`).
Current implementation includes an in-room paid reservation flow (`/buyname [NAME]`) with supernode + 2-of-N ack verification, plus title-screen blocking/warnings for cached reserved names owned by a different local identity. Global portable ownership still requires the on-chain contract layer.

### Crypto-Based Backend (Lightweight)

For features requiring persistence without dedicated servers, consider:

- **Leaderboards**: Could use a decentralized storage solution or crypto-incentivized nodes
- **Player profiles**: On-chain or IPFS-based persistence
- **Session state**: Lightweight signaling could piggyback on existing crypto infrastructure
- **Rewards/achievements**: NFTs or on-chain badges

This would maintain the P2P philosophy while enabling persistent features. Implementation deferred until core gameplay is solid.

### Networking Consistency Enhancements

CRDTs and simple consensus are worth considering only for low-rate, durable shared state, not per-frame enemy positions. Enemy motion should stay prediction + authority correction because consensus on every local movement update would add latency and bandwidth exactly where the game needs immediacy.

Potential future uses:
- **CRDTs**: eventually consistent room metadata, chat/moderation state, or other non-combat state where concurrent edits should merge instead of picking a single winner.
- **Small quorum/consensus checks**: tournament results, paid unlock validation, authority handoff/fencing tokens, or suspicious kill/death outcomes where correctness matters more than frame-level latency.
- **Authority-map hardening**: root-issued leases or monotonically increasing fencing epochs per area so receivers can reject stale enemy authorities without needing heavyweight consensus among every nearby node.

### Future Upgrades + Competitive Prizes
- Paid upgrades could include a forward laser attack, stronger shields (e.g., 3/4 coverage), or other ability enhancements.
- Competitive modes could award crypto prizes (e.g., last-slime-standing vs. environment, or PvP tournaments/gladiator fights).

### Mobile-Friendly UX
- Add touch controls (virtual stick + action buttons), responsive HUD layout, and safe areas for chat/map/player list.

## License

MIT

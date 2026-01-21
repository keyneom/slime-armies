# Slime Armies

A P2P massively multiplayer online game inspired by "One Slime Army". Fight waves of enemies in an infinite procedurally generated world with other players.

Inspired by the original "One Slime Army" (WASM-4).

## Controls

- **WASD / Arrow Keys**: Move
- **Z / Space**: Attack (hold to block, release to swing)
- **X / Shift**: Dodge
- **M**: Open map (arrows pan, Z zoom in, X zoom out, type coords, Enter to teleport)

## Development

```bash
# Install dependencies
rustup target add wasm32-unknown-unknown
cargo install trunk

# Build for GitHub Pages (docs/)
./scripts/build-docs.sh

# Run dev server
trunk serve

# Build for production
trunk build --release
```

## Architecture

- **Rust + WebAssembly**: Core game logic
- **Canvas 2D**: Rendering
- **Matchbox + GGRS**: P2P networking with rollback netcode (planned)
- **Hybrid P2P topology**: Supernodes for 100+ player scaling (planned)

## Roadmap

- [x] Phase 1: Core single-player (player movement, combat)
- [x] Phase 2: Enemies (spider, cannon, snake boss)
- [x] Phase 3: Infinite world with chunk loading
- [x] Phase 4: Basic P2P multiplayer
- [ ] Phase 5: GGRS rollback netcode
- [ ] Phase 6: Supernode architecture for scale
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
If we keep this backendless, the best practical guardrail is host-side verification plus on-chain proofs (e.g., tx hash or signed receipt in the event payload), but it is not cheat-proof without an authoritative server.

We’ll need a generic, immutable smart contract that can verify paid unlocks for any feature (e.g., obstacle drops) without adding new methods per feature. It should also support prize payouts for tournaments and other competitive modes; winner determination likely needs its own consensus process, so contract hooks should expect externally agreed results.

### Crypto-Based Backend (Lightweight)

For features requiring persistence without dedicated servers, consider:

- **Leaderboards**: Could use a decentralized storage solution or crypto-incentivized nodes
- **Player profiles**: On-chain or IPFS-based persistence
- **Session state**: Lightweight signaling could piggyback on existing crypto infrastructure
- **Rewards/achievements**: NFTs or on-chain badges

This would maintain the P2P philosophy while enabling persistent features. Implementation deferred until core gameplay is solid.

### Future Upgrades + Competitive Prizes
- Paid upgrades could include a forward laser attack, stronger shields (e.g., 3/4 coverage), or other ability enhancements.
- Competitive modes could award crypto prizes (e.g., last-slime-standing vs. environment, or PvP tournaments/gladiator fights).

### Mobile-Friendly UX
- Add touch controls (virtual stick + action buttons), responsive HUD layout, and safe areas for chat/map/player list.

## License

MIT

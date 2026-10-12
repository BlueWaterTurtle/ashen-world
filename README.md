# ashen-world
A Rust-built 3D-ready sandbox simulation engine prototype for a brutal open-world game.


## Vision
A brutal, Kenshi-style 3D sandbox: a generated open world where every unit is its own entity, with building, farming, factions and captivity systems. The engine is simulation-first: a deterministic, headless-testable core with rendering as a thin reader of state.

## Milestone 1 scope
Workspace, window + fixed-timestep loop, camera stub, placeholder renderer (top-down dots), seeded world generation, chunked coordinates, movement system, JSON content loading, tests and CI.

## Workspace
- `crates/core_sim` – headless deterministic simulation (RNG, chunk coordinates, world, movement)
- `crates/game_client` – winit window, input, camera, placeholder rendering, debug stats (stdout)
- `crates/content` – serde data models (faction, unit archetype, biome) and validated JSON loader
- `assets/content/*.json` – example content

## Run
```sh
cargo run -p game_client --release   # WASD pan, wheel zoom, Esc quit
```
Frame time, sim ticks/sec and entity count print to stdout once per second. Requires a desktop display.

## Test / lint
```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Roadmap (Milestone 2)
Milestone 2 (done): seeded terrain heightmaps, chunk streaming and grid A* in `core_sim`. Milestone 3 (headless part done): wandering agents with budgeted path requests. Next: real GPU rendering (wgpu) with terrain meshes, needs/inventory, content-driven archetypes.


## Data-driven content
Gameplay values live in `assets/content/*.json`, not the engine: `factions`, `units`, `biomes`, `races` (base stats plus buff/debuff modifiers) and `behaviors` (wander range, pause time, speed multiplier). Units reference a race and behavior by id; references are validated on load. `core_sim::GameClock` derives day/time-of-day from the tick counter (no per-tick cost) for future crop growth.

`buildings.json` and `crops.json` define placeable buildings (footprint, required resource, dwelling, cost, crop) and crops (growth days, yield); `core_sim::Buildings` validates placement (slope, overlap, protected city zones, resource requirement) and crop growth is computed from the clock on demand.

Planned next: ECS migration (hecs), levels/skills, static map + cities, LOD scheduling, wgpu renderer.

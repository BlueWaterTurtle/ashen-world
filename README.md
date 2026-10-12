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
cargo run -p game_client --release   # wgpu 3D: WASD pan, wheel zoom, right/middle-drag orbit, Q/E rotate, Esc quit
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

`buildings.json` and `crops.json` define placeable buildings (footprint, required site quality, worker/input/output slots, accepted resources, cost, crop) and crops (growth days, yield); `core_sim::Buildings` validates placement and provides typed worker assignment, production input/output, and building-to-storage transfer APIs. Farms output wheat, mines output iron ore, and dedicated stores accept each resource. Production scheduling and automatic hauling are not yet run by the simulation.

Simulation runs on `hecs`, maths on `glam`, rendering on `wgpu` (lit, fogged, chunked terrain meshes + instanced agents).

Planned next: levels/skills, static map + cities, LOD scheduling, shadows, glTF models.

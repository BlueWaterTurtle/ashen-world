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
cargo run -p game_client --release   # WASD pan, Q/E down/up, Esc quit
```
Frame time, sim ticks/sec and entity count print to stdout once per second. Requires a desktop display.

## Test / lint
```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Roadmap (Milestone 2)
Terrain and chunk streaming visuals, pathfinding baseline, real GPU rendering (wgpu), proper ECS if needed.

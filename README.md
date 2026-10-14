# ashen-world
A Rust-built 3D-ready sandbox simulation engine prototype for a brutal open-world game.


## Vision
A harsh, 3D open-world sandbox with generated terrain, persistent agents, building, farming, and factions. The project is engine-first: a deterministic, headless-testable simulation core with rendering and game-specific content kept in separate crates.

## Current implementation
The workspace includes deterministic terrain and world simulation, chunk streaming, pathfinding, simulation tiers for distant agents, content-driven buildings and crops, and a WGPU client with orbit camera, lit terrain, instanced characters, building prefabs, and an egui interface.

## Workspace
- `crates/core_sim` – headless deterministic simulation (RNG, chunk coordinates, world, movement)
- `crates/game_client` – winit client, camera and input, WGPU renderer, egui UI, debug stats (stdout)
- `crates/content` – serde data models (faction, unit archetype, biome) and validated JSON loader
- `crates/asset_pipeline` – reusable OBJ/glTF geometry, base-color texture, and prefab importer, independent of simulation and rendering
- `assets/content/*.json` – example content
- `assets/models/` – character, modular building, texture, and placeholder assets
- `assets/prefabs/buildings/*.json` – reusable, transform-based building compositions

## Run
```sh
cargo run -p game_client --release   # wgpu 3D: WASD pan, wheel zoom, right/middle-drag orbit, Q/E rotate, Esc quit
```
Frame time, sim ticks/sec and entity count print to stdout once per second. Requires a desktop display.
Set `ASHEN_ASSET_DIR` to use another asset root containing `content/` and `models/`; otherwise the workspace `assets/` directory is used.

The client loads `models/Humanoids/Superhero_Male_FullBody.gltf` for agents, with `models/humanoid.obj` as fallback. Static glTF base-color textures render per material. Character animation and normal/roughness maps are not yet used. Terrain uses procedural height and slope coloring plus `models/textures/T_Noise_Terrain.png` sampled in world space for broad ash/dust variation and subtle grain. Buildings first look for `prefabs/buildings/<building-id>.json`; recipes compose reusable glTF parts using translation, Y rotation, and scale. Complete building models are supported as `models/buildings/<building-id>.gltf` or `.obj` fallbacks. The farm retains a field-oriented procedural mesh.

## Test / lint
```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Next steps
The rendering client remains a game-facing application rather than a standalone rendering crate. Likely engine work includes skeletal animation, richer terrain materials, reusable asset/render interfaces, and profiling-driven improvements. Simulation work includes production scheduling and automatic hauling; those systems are not currently run by the simulation.


## Data-driven content
Gameplay values live in `assets/content/*.json`, not the engine: `factions`, `units`, `biomes`, `races` (base stats plus buff/debuff modifiers) and `behaviors` (wander range, pause time, speed multiplier). Units reference a race and behavior by id; references are validated on load. `core_sim::GameClock` derives day/time-of-day from the tick counter (no per-tick cost) for future crop growth.

`buildings.json` and `crops.json` define placeable buildings (footprint, required site quality, worker/input/output slots, accepted resources, cost, crop) and crops (growth days, yield); `core_sim::Buildings` validates placement and provides typed worker assignment, production input/output, and building-to-storage transfer APIs. Farms output wheat, mines output iron ore, and dedicated stores accept each resource. Production scheduling and automatic hauling are not yet run by the simulation.

Simulation uses `hecs`; math uses `glam`; rendering uses `wgpu`. Terrain is streamed as chunk meshes, and visible agents are rendered as instances. The asset pipeline imports static glTF geometry and base-color textures; skeletal animation, normal mapping, and roughness mapping remain future work.

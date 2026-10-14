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

`buildings.json` and `crops.json` define placeable buildings (footprint, required site quality, worker/input/output slots, accepted resources, cost, crop) and crops (growth days, yield); `core_sim::Buildings` validates placement and provides typed worker assignment, production input/output, and building-to-storage transfer APIs. Farms output wheat, mines output iron ore, and dedicated stores accept each resource. Resource items are defined once in `core_sim::ResourceKind::def()` (id and `stack_size`, e.g. ore stacks to 50). Mines (`"mine": true`) have an output slot bounded by the item's stack size and produce 1 ore per `ORE_SECONDS_PER_UNIT` (2 s) of worked time via `Buildings::tick_production(dt, is_working)`; production pauses while the slot is full and resumes once space is freed. Workers get a prioritized job list (job panel in the HUD; top job wins, falling through when blocked). `core_sim::Jobs` re-checks every 0.5 s and only visits workers with jobs: a worker takes a building's worker slot (`worker_slots` in the def; production buildings must have at least one), walks to it, and is held there (no wandering, placeholder bob animation) while the building is ready. A full output or a missing `required_inputs` resource blocks the building; the worker leaves the slot but keeps the job and returns once the building can run again. An explicit player move order suspends the worker's jobs until resumed in the panel. Automatic hauling and wider production scheduling are not yet run by the simulation.

Simulation uses `hecs`; math uses `glam`; rendering uses `wgpu`. Terrain is streamed as chunk meshes, and visible agents are rendered as instances. The asset pipeline imports static glTF geometry and base-color textures; skeletal animation, normal mapping, and roughness mapping remain future work.

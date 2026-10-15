# Ashen World

Ashen World is a work-in-progress Rust game prototype about managing a small
group of survivors in a procedurally generated world. It combines a real-time
simulation with a 3D desktop client; it is not yet a finished game.

## Current features

- A deterministic, fixed-step simulation with seeded terrain, chunk streaming,
  pathfinding, and simulation tiers for NPCs.
- Player-controlled characters, recruitable NPCs, squad selection, and movement
  orders.
- Placeable buildings with worker jobs, prioritized job lists, resource
  logistics, and production.
- Editable terrain, including raise, lower, flatten, smooth, and slope tools.
- A `wgpu` renderer and an `egui` HUD for squad, building, and job management.
- Data-driven factions, units, biomes, races, behaviors, buildings, and crops.

Gameplay content is defined in [`assets/content/`](assets/content/) as `.ron`
files. The workspace also includes the headless simulation, content-loading,
and asset-pipeline crates.

## Run

With a stable Rust toolchain installed, run the desktop client from the
repository root:

```sh
cargo run --release -p game_client
```

The client needs a desktop display supported by `winit` and `wgpu`.

## Controls

| Input | Action |
| --- | --- |
| `W` / `A` / `S` / `D` or arrow keys | Pan the camera |
| `Q` / `E` | Rotate the camera |
| Mouse wheel | Zoom |
| Right- or middle-drag | Rotate the camera |
| Left-click a character | Select it; click a recruitable character with a selected recruiter nearby to recruit it |
| Left-drag | Select multiple player characters |
| Left-click empty terrain | Move the selected character or squad |
| `1` / `2` / `3` | Select one of the first three player characters |
| `Space` | Center the camera on the selected character |
| `R` | Recruit the nearest recruitable character within range |
| Select a building in the HUD, then left-click terrain | Place that building. Every building except the iron mine costs 1 iron ore: it becomes a construction site, and idle survivors fetch ore from stocks within 30 m and build it |
| `T` | Toggle terrain editing |
| `F1`–`F5` while editing | Raise, lower, flatten, smooth, or slope |
| `[` / `]` while editing | Decrease / increase brush radius |
| `-` / `=` while editing | Decrease / increase brush strength |
| `Esc` | Exit terrain editing or cancel building placement; otherwise quit |

The HUD provides building selection and worker-job assignment and management.

## Workspace

- `crates/core_sim` — headless world, terrain, NPC, building, and job simulation.
- `crates/game_client` — desktop application, rendering, camera, input, and HUD.
- `crates/content` — serializable content definitions and RON loading.
- `crates/asset_pipeline` — model, texture, and building-prefab loading.
- `assets/` — gameplay content, prefabs, and visual assets.

## Checks

The CI workflow runs formatting, Clippy, and the workspace test suite:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

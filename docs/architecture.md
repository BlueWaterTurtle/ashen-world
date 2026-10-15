# Architecture

## Layers

| Layer | Crate / path | Responsibility |
|---|---|---|
| Simulation core | `crates/core_sim` | Headless, deterministic world model (terrain, NPCs, buildings, jobs, production, clock) and the `SimSystem`/`Schedule` boundary. No rendering, no file I/O. |
| Content | `crates/content` | Data models for content domains, RON loading, validation, cross-reference checks, `manifest.ron`, and the layered registry/cache for mods. |
| Asset pipeline | `crates/asset_pipeline` | Importing assets and loading prefabs (`assets/prefabs/**/*.ron`). |
| Client | `crates/game_client` | Rendering, input, UI. Wires the other crates together. |
| Data | `assets/` | `assets/content/*.ron`, `assets/content/manifest.ron`, `assets/prefabs/`. The source of truth for game data. |

## Allowed dependency directions

```
game_client ──► core_sim
game_client ──► content
game_client ──► asset_pipeline
```

- `core_sim`, `content` and `asset_pipeline` must not depend on `game_client`.
- `core_sim` must not depend on `content` or `asset_pipeline`; the client converts content into
  sim types (e.g. content behaviors into `core_sim::Behavior`).
- `content` must not depend on `core_sim`. Systems appear in the manifest only as names.
- Lower crates never depend on each other sideways unless this document is updated first.

## Content loading

- **Preferred API:** `content::load_content(dir)` loads every domain from a flat directory of
  `<domain>.ron` files and returns a validated `Content`.
- **Layered registry/cache** (`content::registry`, `content::cache`) is the mod-overlay mechanism
  (`assets/content/base/<kind>/` then `mods/*/<kind>/`). It is deliberately kept separate: it
  does no validation and is not used by the game yet. When mods are wired in, it should feed
  `load_content` rather than being called directly by consumers.
- Each domain lives in `crates/content/src/domains/<domain>.rs` and implements `ContentDomain`:
  its file name, kind label, per-record `validate`, and declared `references` to other domains.
  Cross-references are resolved generically by `KnownIds`, so there is no per-pair validation code.

## Runtime composition (manifest)

`assets/content/manifest.ron` is the composition entry point. At startup `game_client`:

1. Loads it with `content::Manifest::load_or_default`. If it is missing or invalid, a warning is
   printed and the built-in default is used (all content domains, `core_step`). Omitted fields
   also take those defaults.
2. Calls `content::load_content_with(dir, &manifest)`, which reads and validates only the
   domains in `content_domains`. Disabled domains stay empty and references into them are not
   checked. `load_content(dir)` is the same with the default manifest.
3. Builds the tick schedule with `core_sim::Schedule::from_names_or_standard(&manifest.systems)`.
   Systems run in the listed order; unknown names are reported and skipped; if none are valid,
   `core_step` is used.

To disable a domain or system, remove its name from the manifest. Add one by following below.

## Adding a content domain

1. Add `crates/content/src/domains/<name>.rs` with the type and a `ContentDomain` impl
   (override `validate` and `references` as needed).
2. Re-export it from `domains/mod.rs` and `lib.rs`.
3. Add a field to `Content`, load it with a `domain!` line in `load_content_with`, and register it
   with `KnownIds::register`; call `check_refs` if it declares references. Add its name to
   `default_domains` in `manifest.rs`.
4. Add `assets/content/<name>.ron` and list `<name>` in `assets/content/manifest.ron`.
5. Add tests (valid load, invalid values, dangling reference).

## Adding a simulation system

1. Implement `core_sim::SimSystem` (state lives in the system, world data in `World`).
2. Register its name in `Schedule::from_names` and list the name in `manifest.ron`.
3. Keep systems deterministic: no wall-clock time, no unseeded randomness.

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
- **Mods** feed the same path: `content::load_content_with_mods(dir, &manifest, mods_dir)` loads
  base content, overlays the mods enabled in `manifest.mods`, and validates the merged result
  (uniqueness, per-record `validate`, cross-domain references). Consumers never call the
  layered registry directly. `content::registry`/`content::cache` (a single-domain
  `base/<kind>/` + `mods/*/<kind>/` mechanism) remain separate and unused by the game; mod
  overlays are merged on the validated `Vec`s, not through them.

## Mods

- **Layout:** `assets/mods/<mod_id>/mod.ron` (`id` must equal the directory name and use only
  `[A-Za-z0-9_-]`; `name`; optional `version`, `load_order` (default 0), `dependencies`) plus
  optional `<domain>.ron` files in the same format as `assets/content`. See
  `assets/mods/example_mod` and `assets/scenarios/example_mod.ron`.
- **Enabling:** `mods: [...]` in the manifest, or `mods: Some([...])` in a scenario (replaces the
  manifest list, like the other scenario lists). Omitted = no mods, so default behavior is unchanged.
- **Order:** mods apply after base content, sorted by `(load_order, id)`; the order in the enable
  list is irrelevant, so results never depend on file-system order.
- **Overlay rules:** a record whose id already exists (in base or an earlier mod) replaces it in
  place (override); a new id is appended. Duplicate ids within one layer are an error. Removal is
  not supported. Only domains enabled in the manifest are read from mods.
- **Dependencies:** each listed dependency must be enabled and loaded *before* the mod (lower
  `load_order`), otherwise the mod is skipped.
- **Failure behavior:** after each mod is merged, the whole content set is re-validated, so mod
  records may reference base records and base records may reference mod records. A mod that is
  missing, has an invalid descriptor/RON, fails validation (errors name the mod's file), or has
  unmet dependencies is skipped and the content before it is kept. Startup never fails because of
  a mod; skips are reported in the `StartupSummary`. Base content errors behave as before.

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

## Scenarios (game modes)

A scenario (`assets/scenarios/<name>.ron`, parsed by `content::Scenario`) sits on top of the
manifest. All fields are optional: `name`, `description`, `manifest` (file relative to
`assets/content`, default `manifest.ron`), inline `content_domains` / `systems` overrides, and
`world` startup overrides (`seed`, `agent_count`, `chunk_radius`, `max_path_requests_per_tick`,
`starting_player_characters`).

At startup `game_client` loads `assets/scenarios/default.ron`, or the scenario named by the
`ASHEN_SCENARIO` environment variable (a name, or a path to a `.ron` file). The scenario resolves
the effective manifest (manifest file, then inline overrides) which drives content and schedule
composition as above; `world` overrides are then applied to `WorldConfig`. If the scenario is
missing or invalid a warning is printed and the built-in default (no overrides) is used, so the
default scenario reproduces the original behavior. Example: `ASHEN_SCENARIO=small_world`.

## Startup resolution order

Composition lives in `game_client::bootstrap` (no winit/wgpu) and runs in this order:

1. **Scenario selected** – `ASHEN_SCENARIO` (name or path, default `default`). Missing/invalid file: built-in default scenario.
2. **Manifest resolved** – the scenario's `manifest` (else `manifest.ron`), then inline `content_domains`/`systems` replace the manifest's lists. Missing/invalid manifest: default manifest.
3. **Content loaded** – only enabled domains, then enabled mods overlaid (`assets/mods`, see Mods) and the merged result validated; bad mods are skipped. Failure of base content: default `WorldConfig` and no building defs. References into disabled domains are skipped and reported as warnings.
4. **Schedule built** – from manifest systems. Unknown names are ignored; an empty result uses the standard schedule.
5. **World overrides applied** – scenario `world` values on top of the config (also after a content fallback).

A structured `StartupSummary` printed at boot lists scenario (requested vs fallback), manifest, domains, systems, loaded mods, skipped mods with reasons, overrides (which mod replaced which id), any fallbacks and warnings.

## Save/load

The simulation core exposes a serde-backed `WorldSnapshot`; it does not read or write files.
Snapshots preserve the tick and RNG state, active NPC component and path state, terrain deltas
and queued edits, behavior/LOD configuration, focus points, and deterministic agent scheduling
queues. The client save envelope also preserves buildings (including inventories and production
progress) and jobs (including active worker assignments). Terrain is regenerated from its seed
and saved edits; walkability/pathfinder caches and rendered/streamed terrain chunks are derived
and rebuilt. The current game clock is derived from the saved tick.

Save files are RON envelopes with `format_version: 1`, the world snapshot, and metadata containing
the selected scenario selector/name, manifest source, enabled domains, systems, loaded mod ids and
versions, and a Unix timestamp. Writes use a same-directory temporary file followed by rename.
This first pass rejects any unsupported format version; it does not migrate older formats.

At startup, `ASHEN_LOAD` accepts a save name (looked up as `saves/<name>.ron`) or a file path.
Bootstrap recomposes the scenario recorded in the save before restoring it, then checks the
scenario, manifest source, domains, systems, and loaded mod ids/versions against the save metadata.
An unreadable, invalid, unsupported, or incompatible save is reported in `StartupSummary` and
startup continues with a fresh world. Without `ASHEN_LOAD`, startup behavior is unchanged. During
play, **F5** writes `saves/quicksave.ron` and **F9** loads it when its composition matches the
current startup. The default `saves/` directory is gitignored.

## Production chain (mine -> haul -> smelt)

A mine worked by a `Job::Work` worker moves ore into its output storage. A `Job::Haul { from, to, item }`
worker carries up to `HAUL_CARRY` items from `from`'s output to `to`'s input (the carried stack
lives in the worker inventory, so it is saved with the world). A building with `smelter: true`
(the workshop) converts one `iron_ore` into one `iron_ingot` per `SMELT_SECONDS_PER_INGOT` worked
seconds. All of it is stored in `Buildings`/`Jobs` and round-trips through save/load. The job panel
shows haul jobs and per-building input/output stock.

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

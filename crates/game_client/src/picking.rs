//! Pure screen-space and frustum helpers for culling and selecting agents.

use core_sim::{World, WorldPos};
use glam::{Mat4, Vec3, Vec4};

const AGENT_CULL_RADIUS: f32 = 1.0;

pub fn agent_in_frustum(position: WorldPos, view_proj: Mat4) -> bool {
    let center = Vec3::new(position.x, position.y + 0.9, position.z);
    let x = Vec4::new(
        view_proj.x_axis.x,
        view_proj.y_axis.x,
        view_proj.z_axis.x,
        view_proj.w_axis.x,
    );
    let y = Vec4::new(
        view_proj.x_axis.y,
        view_proj.y_axis.y,
        view_proj.z_axis.y,
        view_proj.w_axis.y,
    );
    let z = Vec4::new(
        view_proj.x_axis.z,
        view_proj.y_axis.z,
        view_proj.z_axis.z,
        view_proj.w_axis.z,
    );
    let w = Vec4::new(
        view_proj.x_axis.w,
        view_proj.y_axis.w,
        view_proj.z_axis.w,
        view_proj.w_axis.w,
    );
    [w + x, w - x, w + y, w - y, z, w - z].iter().all(|plane| {
        let normal = plane.truncate();
        normal.dot(center) + plane.w >= -AGENT_CULL_RADIUS * normal.length()
    })
}

pub fn select_players_in_rect(
    world: &World,
    view_proj: Mat4,
    width: f32,
    height: f32,
    start: (f32, f32),
    end: (f32, f32),
) -> Vec<core_sim::Entity> {
    let left = start.0.min(end.0);
    let right = start.0.max(end.0);
    let top = start.1.min(end.1);
    let bottom = start.1.max(end.1);
    world
        .player_characters()
        .filter(|&entity| {
            let position = world.position(entity);
            let clip = view_proj * Vec3::new(position.x, position.y + 0.9, position.z).extend(1.0);
            if clip.w <= 0.0 {
                return false;
            }
            let ndc = clip.truncate() / clip.w;
            if !(0.0..=1.0).contains(&ndc.z) {
                return false;
            }
            let screen_x = (ndc.x + 1.0) * 0.5 * width;
            let screen_y = (1.0 - ndc.y) * 0.5 * height;
            (left..=right).contains(&screen_x) && (top..=bottom).contains(&screen_y)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::Camera;
    use core_sim::WorldConfig;

    #[test]
    fn agent_frustum_culling_keeps_visible_and_discards_distant_agents() {
        let camera = Camera::default();
        let view_proj = camera.view_proj();
        let target = camera.target;
        assert!(agent_in_frustum(
            WorldPos::new(target.x, target.y, target.z),
            view_proj
        ));
        assert!(!agent_in_frustum(
            WorldPos::new(target.x + 100_000.0, target.y, target.z),
            view_proj
        ));
    }

    #[test]
    fn drag_selection_includes_only_player_characters_inside_screen_rect() {
        use core_sim::Entity;

        let camera = Camera::default();
        let target = camera.target;
        let mut world = World::generate(&WorldConfig {
            seed: 123,
            agent_count: 4,
            starting_player_characters: 3,
            ..Default::default()
        });
        world.set_position(Entity(0), WorldPos::new(target.x - 1.0, target.y, target.z));
        world.set_position(Entity(1), WorldPos::new(target.x + 1.0, target.y, target.z));
        world.set_position(
            Entity(2),
            WorldPos::new(target.x + 100.0, target.y, target.z),
        );

        let project = |entity| {
            let position = world.position(entity);
            let clip = camera.view_proj()
                * Vec3::new(position.x, position.y + 0.9, position.z).extend(1.0);
            let ndc = clip.truncate() / clip.w;
            ((ndc.x + 1.0) * 400.0, (1.0 - ndc.y) * 300.0)
        };
        let first = project(Entity(0));
        let second = project(Entity(1));
        let start = (first.0.min(second.0) - 10.0, first.1.min(second.1) - 10.0);
        let end = (first.0.max(second.0) + 10.0, first.1.max(second.1) + 10.0);
        let selected = select_players_in_rect(&world, camera.view_proj(), 800.0, 600.0, start, end);

        assert_eq!(selected, vec![Entity(0), Entity(1)]);
    }

    #[test]
    fn rendering_includes_reduced_and_excludes_dormant_and_culled_agents() {
        use core_sim::{Entity, SimulationTier};

        let camera = Camera::default();
        let view_proj = camera.view_proj();
        let target = camera.target;

        let mut world = World::generate(&WorldConfig {
            seed: 123,
            agent_count: 4,
            ..Default::default()
        });

        let e_active = Entity(0);
        let e_reduced = Entity(1);
        let e_dormant = Entity(2);
        let e_culled = Entity(3);

        world.set_tier(e_active, SimulationTier::Active);
        world.set_position(e_active, WorldPos::new(target.x, target.y, target.z));

        // Reduced tier: simulated less often, but STILL rendered when visible
        world.set_tier(e_reduced, SimulationTier::Reduced);
        world.set_position(e_reduced, WorldPos::new(target.x + 1.0, target.y, target.z));

        // Dormant tier: not rendered until promoted back to higher tier
        world.set_tier(e_dormant, SimulationTier::Dormant);
        world.set_position(e_dormant, WorldPos::new(target.x + 2.0, target.y, target.z));

        // Active tier, but outside camera frustum
        world.set_tier(e_culled, SimulationTier::Active);
        world.set_position(
            e_culled,
            WorldPos::new(target.x + 50_000.0, target.y, target.z),
        );

        let mut rendered_entities = Vec::new();
        for e in world.entities() {
            if !world.is_renderable(e) {
                continue;
            }
            let pos = world.position(e);
            if agent_in_frustum(pos, view_proj) {
                rendered_entities.push(e);
            }
        }

        assert!(rendered_entities.contains(&e_active));
        assert!(rendered_entities.contains(&e_reduced));
        assert!(!rendered_entities.contains(&e_dormant));
        assert!(!rendered_entities.contains(&e_culled));
    }
}

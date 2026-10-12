use glam::{Mat4, Vec3};

/// Orbit camera around a ground target: yaw/pitch/distance plus projection parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub target: Vec3,
    /// Rotation around the Y axis in radians.
    pub yaw: f32,
    /// Elevation above the horizon in radians.
    pub pitch: f32,
    /// Distance from the target to the eye in meters.
    pub distance: f32,
    pub fov_y_radians: f32,
    pub aspect: f32,
    pub near: f32,
    pub far: f32,
}

pub const MIN_DISTANCE: f32 = 6.0;
pub const MAX_DISTANCE: f32 = 320.0;
const MIN_PITCH: f32 = 0.12;
const MAX_PITCH: f32 = 1.5;

impl Default for Camera {
    fn default() -> Self {
        Self {
            target: Vec3::new(32.0, 0.0, 32.0),
            yaw: 0.6,
            pitch: 0.75,
            distance: 90.0,
            fov_y_radians: 50f32.to_radians(),
            aspect: 16.0 / 9.0,
            near: 0.5,
            far: 3000.0,
        }
    }
}

impl Camera {
    pub fn eye(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        self.target + self.distance * Vec3::new(cp * sy, sp, cp * cy)
    }

    pub fn view_proj(&self) -> Mat4 {
        let view = Mat4::look_at_rh(self.eye(), self.target, Vec3::Y);
        let proj = Mat4::perspective_rh(self.fov_y_radians, self.aspect, self.near, self.far);
        proj * view
    }

    /// Zoom by mouse-wheel steps (positive = in), clamped to sensible limits.
    pub fn zoom(&mut self, steps: f32) {
        self.distance = (self.distance * 0.88f32.powf(steps)).clamp(MIN_DISTANCE, MAX_DISTANCE);
    }

    /// Orbit by yaw/pitch deltas in radians.
    pub fn orbit(&mut self, dyaw: f32, dpitch: f32) {
        self.yaw = (self.yaw + dyaw).rem_euclid(std::f32::consts::TAU);
        self.pitch = (self.pitch + dpitch).clamp(MIN_PITCH, MAX_PITCH);
    }

    /// Pan the target on the ground plane, relative to the view direction.
    pub fn pan(&mut self, right: f32, forward: f32) {
        let (sy, cy) = self.yaw.sin_cos();
        let fwd = Vec3::new(-sy, 0.0, -cy);
        let rgt = Vec3::new(cy, 0.0, -sy);
        self.target += rgt * right + fwd * forward;
    }

    /// Cast a ray from screen pixel coordinates into world space.
    pub fn screen_ray(
        &self,
        screen_x: f32,
        screen_y: f32,
        width: f32,
        height: f32,
    ) -> (Vec3, Vec3) {
        let ndc_x = (2.0 * screen_x / width) - 1.0;
        let ndc_y = 1.0 - (2.0 * screen_y / height);
        let inv_view_proj = self.view_proj().inverse();
        let near = inv_view_proj.project_point3(Vec3::new(ndc_x, ndc_y, 0.0));
        let far = inv_view_proj.project_point3(Vec3::new(ndc_x, ndc_y, 1.0));
        let dir = (far - near).normalize();
        (near, dir)
    }

    /// Intersect a screen pixel ray with a horizontal ground plane `y = y_plane`.
    pub fn ground_intersection(
        &self,
        screen_x: f32,
        screen_y: f32,
        width: f32,
        height: f32,
        y_plane: f32,
    ) -> Option<Vec3> {
        let (origin, dir) = self.screen_ray(screen_x, screen_y, width, height);
        if dir.y.abs() < 1e-6 {
            return None;
        }
        let t = (y_plane - origin.y) / dir.y;
        if t < 0.0 {
            return None;
        }
        Some(origin + dir * t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_center_ray_intersects_ground_near_target() {
        let c = Camera::default();
        let hit = c
            .ground_intersection(400.0, 300.0, 800.0, 600.0, c.target.y)
            .expect("center ray hits ground plane");
        assert!((hit.x - c.target.x).abs() < 1e-2, "hit.x = {}", hit.x);
        assert!((hit.z - c.target.z).abs() < 1e-2, "hit.z = {}", hit.z);
    }

    #[test]
    fn eye_is_at_orbit_distance() {
        let c = Camera::default();
        assert!(((c.eye() - c.target).length() - c.distance).abs() < 1e-3);
    }

    #[test]
    fn zoom_is_clamped_and_directional() {
        let mut c = Camera::default();
        let d = c.distance;
        c.zoom(1.0);
        assert!(c.distance < d);
        c.zoom(1000.0);
        assert_eq!(c.distance, MIN_DISTANCE);
        c.zoom(-1000.0);
        assert_eq!(c.distance, MAX_DISTANCE);
    }

    #[test]
    fn orbit_clamps_pitch() {
        let mut c = Camera::default();
        c.orbit(0.0, 10.0);
        assert_eq!(c.pitch, MAX_PITCH);
        c.orbit(0.0, -10.0);
        assert_eq!(c.pitch, MIN_PITCH);
    }

    #[test]
    fn pan_forward_moves_toward_view_direction() {
        let mut c = Camera::default();
        let before = c.target;
        let away = c.eye() - c.target;
        c.pan(0.0, 5.0);
        let moved = c.target - before;
        assert!(moved.dot(Vec3::new(away.x, 0.0, away.z)) < 0.0);
        assert!((moved.length() - 5.0).abs() < 1e-4);
    }

    #[test]
    fn target_projects_to_screen_center() {
        let c = Camera::default();
        let p = c.view_proj() * c.target.extend(1.0);
        let ndc = p.truncate() / p.w;
        assert!(ndc.x.abs() < 1e-4 && ndc.y.abs() < 1e-4);
        assert!((0.0..1.0).contains(&ndc.z));
    }
}

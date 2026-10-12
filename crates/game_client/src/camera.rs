/// 3D-ready camera: position, yaw/pitch orientation and projection parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    pub position: [f32; 3],
    /// Rotation around Y in radians.
    pub yaw: f32,
    /// Rotation around X in radians.
    pub pitch: f32,
    pub fov_y_radians: f32,
    pub aspect: f32,
    pub near: f32,
    pub far: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            position: [0.0, 80.0, 0.0],
            yaw: 0.0,
            pitch: -std::f32::consts::FRAC_PI_2,
            fov_y_radians: 60f32.to_radians(),
            aspect: 16.0 / 9.0,
            near: 0.1,
            far: 5000.0,
        }
    }
}

impl Camera {
    /// Unit forward vector from yaw/pitch (used by future 3D rendering).
    #[allow(dead_code)]
    pub fn forward(&self) -> [f32; 3] {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        [sy * cp, sp, -cy * cp]
    }

    /// Move along the ground plane (right/forward in camera yaw) and vertically.
    pub fn pan(&mut self, right: f32, forward: f32, up: f32) {
        let (sy, cy) = self.yaw.sin_cos();
        self.position[0] += right * cy + forward * sy;
        self.position[2] += right * sy - forward * cy;
        self.position[1] += up;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_looks_down() {
        let f = Camera::default().forward();
        assert!(f[1] < -0.99);
    }

    #[test]
    fn pan_forward_at_zero_yaw_moves_negative_z() {
        let mut c = Camera::default();
        c.pan(0.0, 1.0, 0.0);
        assert!(c.position[2] < 0.0);
    }
}

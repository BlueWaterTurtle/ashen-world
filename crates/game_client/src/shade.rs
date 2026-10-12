use core_sim::height_at;

/// Sun direction (normalised, y up) used for hillshading; low sun gives strong relief.
const SUN: [f32; 3] = [-0.5, 0.45, -0.6];

fn lerp(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// Lit terrain colour (0x00RRGGBB) at a world position: height-banded base colour
/// (ash -> dust -> rock) modulated by a sun term from the surface normal, with steep
/// slopes pushed toward bare rock.
pub fn terrain_color(seed: u64, x: f32, z: f32) -> u32 {
    const EPS: f32 = 1.0;
    let h = height_at(seed, x, z);
    let dhdx = (height_at(seed, x + EPS, z) - height_at(seed, x - EPS, z)) / (2.0 * EPS);
    let dhdz = (height_at(seed, x, z + EPS) - height_at(seed, x, z - EPS)) / (2.0 * EPS);
    let len = (dhdx * dhdx + 1.0 + dhdz * dhdz).sqrt();
    let n = [-dhdx / len, 1.0 / len, -dhdz / len];
    let sl = (SUN[0] * SUN[0] + SUN[1] * SUN[1] + SUN[2] * SUN[2]).sqrt();
    let diffuse = ((n[0] * SUN[0] + n[1] * SUN[1] + n[2] * SUN[2]) / sl).max(0.0);
    let light = 0.35 + 0.9 * diffuse;

    let t = (h / 40.0).clamp(0.0, 1.0);
    let ash = [0.23, 0.21, 0.20];
    let dust = [0.47, 0.38, 0.27];
    let rock = [0.62, 0.60, 0.57];
    let base = if t < 0.5 {
        lerp(ash, dust, t * 2.0)
    } else {
        lerp(dust, rock, (t - 0.5) * 2.0)
    };
    let steep = ((1.0 - n[1]) * 6.0).clamp(0.0, 1.0);
    let c = lerp(base, [0.4, 0.38, 0.36], steep * 0.6);
    let ch = |v: f32| ((v * light).clamp(0.0, 1.0) * 255.0) as u32;
    (ch(c[0]) << 16) | (ch(c[1]) << 8) | ch(c[2])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_is_deterministic_and_opaque_rgb() {
        let a = terrain_color(1, 10.0, 20.0);
        assert_eq!(a, terrain_color(1, 10.0, 20.0));
        assert_eq!(a >> 24, 0);
    }

    #[test]
    fn terrain_has_visible_relief() {
        let colors: std::collections::HashSet<u32> = (0..64)
            .map(|i| terrain_color(1, i as f32 * 3.0, i as f32 * 5.0))
            .collect();
        assert!(colors.len() > 16, "terrain should not be flat-coloured");
    }
}

//! The Model View's orbit camera.

use glam::{Mat4, Vec2, Vec3};

/// Vertical field of view, in radians.
const FOV_Y: f32 = 0.7;

pub struct Orbit {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub target: Vec3,
}

impl Orbit {
    /// A camera that frames the box from `min` to `max`.
    pub fn framing(min: Vec3, max: Vec3) -> Self {
        let radius = ((max - min).length() * 0.5).max(1.0);
        Self {
            yaw: 0.6,
            pitch: 0.2,
            distance: radius * 2.6,
            target: (min + max) * 0.5,
        }
    }

    /// A camera from the same side as [`Orbit::framing`], as close as it can
    /// be while `points`, which lie between `min` and `max`, fit in a square
    /// view with a small margin.
    pub fn portrait(min: Vec3, max: Vec3, points: impl IntoIterator<Item = Vec3>) -> Self {
        let mut camera = Self::framing(min, max);
        let back = camera.direction();
        let right = (-back).cross(Vec3::Y).normalize();
        let up = right.cross(-back);
        let slope = (FOV_Y * 0.5).tan() / 1.08;
        camera.distance = points
            .into_iter()
            .map(|p| {
                let p = p - camera.target;
                let side = p.dot(right).abs().max(p.dot(up).abs());
                p.dot(back) + side / slope
            })
            .fold(1.0, f32::max);
        camera
    }

    /// From the target toward the eye.
    fn direction(&self) -> Vec3 {
        Vec3::new(
            self.yaw.sin() * self.pitch.cos(),
            self.pitch.sin(),
            self.yaw.cos() * self.pitch.cos(),
        )
    }

    /// The ray from the eye through `ndc`, a point of the view in
    /// normalized device coordinates (-1 to 1, y up): its origin on the
    /// near plane and its unit direction.
    pub fn ray(&self, aspect: f32, ndc: Vec2) -> (Vec3, Vec3) {
        let inverse = self.view_proj(aspect).inverse();
        let near = inverse.project_point3(ndc.extend(0.0));
        let far = inverse.project_point3(ndc.extend(1.0));
        (near, (far - near).normalize())
    }

    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        let eye = self.target + self.direction() * self.distance;
        let view = glam::camera::rh::view::look_at_mat4(eye, self.target, Vec3::Y);
        // wgpu clip space matches the DirectX convention: depth 0..1.
        let proj = glam::camera::rh::proj::directx::perspective(
            FOV_Y,
            aspect,
            self.distance * 0.01,
            self.distance * 10.0,
        );
        proj * view
    }
}

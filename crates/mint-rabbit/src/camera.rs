//! The Model View's orbit camera.

use glam::{Mat4, Vec3};

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

    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        let dir = Vec3::new(
            self.yaw.sin() * self.pitch.cos(),
            self.pitch.sin(),
            self.yaw.cos() * self.pitch.cos(),
        );
        let eye = self.target + dir * self.distance;
        let view = glam::camera::rh::view::look_at_mat4(eye, self.target, Vec3::Y);
        // wgpu clip space matches the DirectX convention: depth 0..1.
        let proj = glam::camera::rh::proj::directx::perspective(
            0.7,
            aspect,
            self.distance * 0.01,
            self.distance * 10.0,
        );
        proj * view
    }
}

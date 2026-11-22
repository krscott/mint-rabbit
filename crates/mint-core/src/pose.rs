use std::collections::BTreeMap;

use crate::NodeId;
use glam::Quat;

/// Display-only rotations applied on top of node transforms. A pose is never
/// exported; it exists so skeletons whose rest rotations come from animation
/// data look right while editing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pose {
    rotations: BTreeMap<NodeId, Quat>,
}

impl Pose {
    #[must_use]
    pub fn rotation(&self, node: NodeId) -> Quat {
        self.rotations.get(&node).copied().unwrap_or(Quat::IDENTITY)
    }

    pub fn set_rotation(&mut self, node: NodeId, rotation: Quat) {
        if rotation == Quat::IDENTITY {
            self.rotations.remove(&node);
        } else {
            self.rotations.insert(node, rotation);
        }
    }
}

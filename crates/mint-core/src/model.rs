//! The node tree: nodes, their transforms, and the meshes they own.

use crate::texture::MaterialId;
use glam::{Mat4, Quat, Vec2, Vec3};

/// Index of a node within its [`Model`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u32);

impl NodeId {
    pub const ROOT: Self = Self(0);

    #[must_use]
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            translation: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        }
    }
}

impl Transform {
    #[must_use]
    pub fn from_translation(translation: Vec3) -> Self {
        Self {
            translation,
            ..Self::default()
        }
    }

    #[must_use]
    pub fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub name: String,
    parent: Option<NodeId>,
    pub transform: Transform,
    pub mesh: Option<Mesh>,
}

impl Node {
    #[must_use]
    pub fn parent(&self) -> Option<NodeId> {
        self.parent
    }
}

/// A tree of nodes.
///
/// Every node except the root has a parent with a smaller index, so the
/// structure is always a tree and a forward pass over ids visits parents
/// first. Ids are not in depth-first order; use [`Model::preorder`] for that.
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    nodes: Vec<Node>,
}

impl Model {
    #[must_use]
    pub fn new(root_name: impl Into<String>, root_transform: Transform) -> Self {
        Self {
            nodes: vec![Node {
                name: root_name.into(),
                parent: None,
                transform: root_transform,
                mesh: None,
            }],
        }
    }

    /// Adds a child as the last node in the tree.
    ///
    /// Panics if `parent` is not a node of this model.
    pub fn add_child(
        &mut self,
        parent: NodeId,
        name: impl Into<String>,
        transform: Transform,
    ) -> NodeId {
        assert!(
            parent.index() < self.nodes.len(),
            "unknown parent {parent:?}"
        );
        let id = NodeId(self.nodes.len() as u32);
        self.nodes.push(Node {
            name: name.into(),
            parent: Some(parent),
            transform,
            mesh: None,
        });
        id
    }

    #[must_use]
    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    pub fn ids(&self) -> impl Iterator<Item = NodeId> + use<> {
        (0..self.nodes.len() as u32).map(NodeId)
    }

    #[must_use]
    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id.index()]
    }

    pub fn node_mut(&mut self, id: NodeId) -> &mut Node {
        &mut self.nodes[id.index()]
    }

    #[must_use]
    pub fn contains(&self, id: NodeId) -> bool {
        id.index() < self.nodes.len()
    }

    pub fn children(&self, id: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        self.ids().filter(move |&c| self.node(c).parent == Some(id))
    }

    /// Every node in depth-first pre-order, children in id order.
    #[must_use]
    pub fn preorder(&self) -> Vec<NodeId> {
        let mut out = Vec::with_capacity(self.nodes.len());
        let mut stack = vec![NodeId::ROOT];
        while let Some(id) = stack.pop() {
            out.push(id);
            let mut children: Vec<_> = self.children(id).collect();
            children.reverse();
            stack.extend(children);
        }
        out
    }

    /// Each node's matrix relative to its parent: its local transform
    /// followed by the pose rotation for that node.
    #[must_use]
    pub fn local_matrices(&self, pose: &crate::Pose) -> Vec<Mat4> {
        self.ids()
            .map(|id| self.node(id).transform.matrix() * Mat4::from_quat(pose.rotation(id)))
            .collect()
    }

    /// Model-space matrix of every node, with each node's local transform
    /// followed by the pose rotation for that node.
    #[must_use]
    pub fn world_matrices(&self, pose: &crate::Pose) -> Vec<Mat4> {
        self.compose(&self.local_matrices(pose))
    }

    /// Model-space matrices from each node's matrix relative to its parent.
    #[must_use]
    pub fn compose(&self, locals: &[Mat4]) -> Vec<Mat4> {
        let mut out: Vec<Mat4> = Vec::with_capacity(self.nodes.len());
        for (node, &local) in self.nodes.iter().zip(locals) {
            let world = match node.parent {
                Some(p) => out[p.index()] * local,
                None => local,
            };
            out.push(world);
        }
        out
    }

    /// Each node's number of ancestors.
    #[must_use]
    pub fn depths(&self) -> Vec<usize> {
        let mut out: Vec<usize> = Vec::with_capacity(self.nodes.len());
        for node in &self.nodes {
            out.push(node.parent.map_or(0, |p| out[p.index()] + 1));
        }
        out
    }

    /// The nodes' meshes, by node.
    pub fn meshes(&self) -> impl Iterator<Item = Option<&Mesh>> {
        self.nodes.iter().map(|n| n.mesh.as_ref())
    }
}

/// Geometry owned by a node.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub triangles: Vec<Triangle>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex {
    /// Position in the local space of the node this vertex follows.
    pub position: Vec3,
    pub normal: Vec3,
    /// The node this vertex follows when it is not the mesh's own node;
    /// such a vertex is a seam vertex.
    pub follows: Option<NodeId>,
}

impl Vertex {
    /// The node this vertex follows, given the node that owns its mesh.
    #[must_use]
    pub fn followed(&self, owner: NodeId) -> NodeId {
        self.follows.unwrap_or(owner)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Triangle {
    pub corners: [Corner; 3],
    pub material: MaterialId,
}

/// One corner of a triangle. UVs live on corners rather than vertices so a
/// vertex can sit on a UV seam or be shared by differently sized textures.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Corner {
    pub vertex: u32,
    /// Normalized texture coordinate; 0..1 spans the texture once.
    pub uv: Vec2,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Pose;

    #[test]
    fn world_matrices_compose_parent_first() {
        let mut model = Model::new(
            "root",
            Transform::from_translation(Vec3::new(0.0, 1.0, 0.0)),
        );
        let child = model.add_child(
            NodeId::ROOT,
            "child",
            Transform::from_translation(Vec3::new(2.0, 0.0, 0.0)),
        );
        let mut pose = Pose::default();
        pose.set_rotation(
            NodeId::ROOT,
            Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
        );

        let world = model.world_matrices(&pose);
        let p = world[child.index()].transform_point3(Vec3::ZERO);

        // The root's rotation turns the child's +X offset into +Y.
        assert!((p - Vec3::new(0.0, 3.0, 0.0)).length() < 1e-5, "{p}");
    }

    #[test]
    fn children_lists_direct_children_in_order() {
        let mut model = Model::new("root", Transform::default());
        let a = model.add_child(NodeId::ROOT, "a", Transform::default());
        let _a1 = model.add_child(a, "a1", Transform::default());
        let b = model.add_child(NodeId::ROOT, "b", Transform::default());

        assert_eq!(model.children(NodeId::ROOT).collect::<Vec<_>>(), vec![a, b]);
    }

    #[test]
    fn preorder_visits_subtrees_before_later_siblings() {
        let mut model = Model::new("root", Transform::default());
        let a = model.add_child(NodeId::ROOT, "a", Transform::default());
        let b = model.add_child(NodeId::ROOT, "b", Transform::default());
        // Added after b, but belongs before it in pre-order.
        let a1 = model.add_child(a, "a1", Transform::default());

        assert_eq!(model.preorder(), vec![NodeId::ROOT, a, a1, b]);
    }
}

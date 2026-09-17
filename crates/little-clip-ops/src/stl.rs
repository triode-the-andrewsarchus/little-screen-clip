//! STL 3D mesh compatibility module.
//!
//! Re-exports core 3D types from [`crate::model3d`], preserving the [`StlMesh`]
//! type alias, vector operations, and format detection helpers.

pub use crate::model3d::{Model3d, ModelFormat, Triangle, Vec3, is_3d_model_file, is_stl_file};

/// Backwards-compatible type alias for [`Model3d`].
pub type StlMesh = Model3d;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stl_compatibility_alias() {
        let ascii_stl = r#"solid simple_triangle
  facet normal 0 0 1
    outer loop
      vertex 0 0 0
      vertex 10 0 0
      vertex 0 10 0
    endloop
  endfacet
endsolid simple_triangle
"#;
        let mesh = StlMesh::parse_bytes(ascii_stl.as_bytes(), "simple".to_string()).unwrap();
        assert_eq!(mesh.triangles.len(), 1);
        assert_eq!(mesh.name, "simple");
        assert_eq!(mesh.triangles[0].v2, Vec3::new(10.0, 0.0, 0.0));
        assert!((mesh.max_dimension - 10.0).abs() < 1e-4);
    }
}

use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    pub fn sub(self, other: Vec3) -> Vec3 {
        Vec3::new(self.x - other.x, self.y - other.y, self.z - other.z)
    }

    pub fn add(self, other: Vec3) -> Vec3 {
        Vec3::new(self.x + other.x, self.y + other.y, self.z + other.z)
    }

    pub fn scale(self, s: f32) -> Vec3 {
        Vec3::new(self.x * s, self.y * s, self.z * s)
    }

    pub fn dot(self, other: Vec3) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    pub fn cross(self, other: Vec3) -> Vec3 {
        Vec3::new(
            self.y * other.z - self.z * other.y,
            self.z * other.x - self.x * other.z,
            self.x * other.y - self.y * other.x,
        )
    }

    pub fn length(self) -> f32 {
        (self.x * self.x + self.y * self.y + self.z * self.z).sqrt()
    }

    pub fn normalize(self) -> Vec3 {
        let l = self.length();
        if l > 1e-6 {
            Vec3::new(self.x / l, self.y / l, self.z / l)
        } else {
            Vec3::new(0.0, 0.0, 1.0)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Triangle {
    pub normal: Vec3,
    pub v1: Vec3,
    pub v2: Vec3,
    pub v3: Vec3,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StlMesh {
    pub name: String,
    pub triangles: Vec<Triangle>,
    pub min_bounds: Vec3,
    pub max_bounds: Vec3,
    pub center: Vec3,
    pub size: Vec3,
    pub max_dimension: f32,
}

pub fn is_stl_file<P: AsRef<Path>>(path: P) -> bool {
    let p = path.as_ref();
    if let Some(ext) = p.extension().and_then(|s| s.to_str()) {
        ext.eq_ignore_ascii_case("stl")
    } else {
        false
    }
}

impl StlMesh {
    pub fn parse_file<P: AsRef<Path>>(path: P) -> Result<Self, String> {
        let mut file = File::open(path.as_ref())
            .map_err(|e| format!("Failed to open STL file: {}", e))?;
        let name = path
            .as_ref()
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Model")
            .to_string();

        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)
            .map_err(|e| format!("Failed to read STL data: {}", e))?;

        Self::parse_bytes(&buffer, name)
    }

    pub fn parse_bytes(buffer: &[u8], default_name: String) -> Result<Self, String> {
        if buffer.len() < 84 {
            return Err("STL file is too short (< 84 bytes)".to_string());
        }

        // Determine whether binary or ASCII
        let trimmed_leading: Vec<u8> = buffer
            .iter()
            .skip_while(|b| b.is_ascii_whitespace())
            .copied()
            .take(6)
            .collect();
        let starts_with_solid = trimmed_leading.starts_with(b"solid");

        let declared_count = if buffer.len() >= 84 {
            u32::from_le_bytes([buffer[80], buffer[81], buffer[82], buffer[83]]) as usize
        } else {
            0
        };
        let expected_binary_len = 84 + declared_count * 50;

        let is_ascii = starts_with_solid
            && (buffer.len() != expected_binary_len || buffer.iter().take(512).all(|&b| b.is_ascii()));

        if is_ascii {
            Self::parse_ascii(buffer, default_name)
        } else {
            Self::parse_binary(buffer, default_name)
        }
    }

    fn parse_binary(buffer: &[u8], name: String) -> Result<Self, String> {
        if buffer.len() < 84 {
            return Err("Invalid binary STL".to_string());
        }

        let num_triangles = u32::from_le_bytes([buffer[80], buffer[81], buffer[82], buffer[83]]) as usize;
        let mut triangles = Vec::with_capacity(num_triangles.min(100_000));

        let mut offset = 84;
        while offset + 50 <= buffer.len() {
            let read_f32 = |off: usize| -> f32 {
                f32::from_le_bytes([buffer[off], buffer[off + 1], buffer[off + 2], buffer[off + 3]])
            };

            let nx = read_f32(offset);
            let ny = read_f32(offset + 4);
            let nz = read_f32(offset + 8);

            let v1 = Vec3::new(read_f32(offset + 12), read_f32(offset + 16), read_f32(offset + 20));
            let v2 = Vec3::new(read_f32(offset + 24), read_f32(offset + 28), read_f32(offset + 32));
            let v3 = Vec3::new(read_f32(offset + 36), read_f32(offset + 40), read_f32(offset + 44));

            let mut normal = Vec3::new(nx, ny, nz);
            if normal.length() < 1e-4 {
                // compute facet normal from cross product of edges
                let edge1 = v2.sub(v1);
                let edge2 = v3.sub(v1);
                normal = edge1.cross(edge2).normalize();
            }

            triangles.push(Triangle { normal, v1, v2, v3 });
            offset += 50;
        }

        Self::build_mesh(name, triangles)
    }

    fn parse_ascii(buffer: &[u8], default_name: String) -> Result<Self, String> {
        let cursor = std::io::Cursor::new(buffer);
        let reader = BufReader::new(cursor);
        let mut triangles = Vec::new();
        let mut current_normal = Vec3::new(0.0, 0.0, 1.0);
        let mut current_vertices: Vec<Vec3> = Vec::with_capacity(3);
        let mut name = default_name;

        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => continue,
            };
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.is_empty() {
                continue;
            }

            match parts[0].to_ascii_lowercase().as_str() {
                "solid" => {
                    if parts.len() > 1 {
                        name = parts[1..].join(" ");
                    }
                }
                "facet" => {
                    if parts.len() >= 5 && parts[1].eq_ignore_ascii_case("normal") {
                        if let (Ok(nx), Ok(ny), Ok(nz)) = (
                            parts[2].parse::<f32>(),
                            parts[3].parse::<f32>(),
                            parts[4].parse::<f32>(),
                        ) {
                            current_normal = Vec3::new(nx, ny, nz).normalize();
                        }
                    }
                    current_vertices.clear();
                }
                "vertex" => {
                    if parts.len() >= 4 {
                        if let (Ok(x), Ok(y), Ok(z)) = (
                            parts[1].parse::<f32>(),
                            parts[2].parse::<f32>(),
                            parts[3].parse::<f32>(),
                        ) {
                            current_vertices.push(Vec3::new(x, y, z));
                        }
                    }
                }
                "endfacet" => {
                    if current_vertices.len() == 3 {
                        let v1 = current_vertices[0];
                        let v2 = current_vertices[1];
                        let v3 = current_vertices[2];
                        let mut norm = current_normal;
                        if norm.length() < 1e-4 {
                            norm = v2.sub(v1).cross(v3.sub(v1)).normalize();
                        }
                        triangles.push(Triangle { normal: norm, v1, v2, v3 });
                    }
                    current_vertices.clear();
                }
                _ => {}
            }
        }

        Self::build_mesh(name, triangles)
    }

    fn build_mesh(name: String, triangles: Vec<Triangle>) -> Result<Self, String> {
        if triangles.is_empty() {
            return Err("STL model contains no triangles".to_string());
        }

        let mut min_x = f32::INFINITY;
        let mut min_y = f32::INFINITY;
        let mut min_z = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        let mut max_z = f32::NEG_INFINITY;

        for t in &triangles {
            for v in &[t.v1, t.v2, t.v3] {
                if v.x < min_x { min_x = v.x; }
                if v.x > max_x { max_x = v.x; }
                if v.y < min_y { min_y = v.y; }
                if v.y > max_y { max_y = v.y; }
                if v.z < min_z { min_z = v.z; }
                if v.z > max_z { max_z = v.z; }
            }
        }

        let min_bounds = Vec3::new(min_x, min_y, min_z);
        let max_bounds = Vec3::new(max_x, max_y, max_z);
        let center = Vec3::new(
            (min_x + max_x) * 0.5,
            (min_y + max_y) * 0.5,
            (min_z + max_z) * 0.5,
        );
        let size = Vec3::new(
            max_x - min_x,
            max_y - min_y,
            max_z - min_z,
        );
        let max_dimension = size.x.max(size.y).max(size.z).max(0.001);

        Ok(Self {
            name,
            triangles,
            min_bounds,
            max_bounds,
            center,
            size,
            max_dimension,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ascii_stl_parsing() {
        let ascii_stl = r#"
solid simple_triangle
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
        assert_eq!(mesh.name, "simple_triangle");
        assert_eq!(mesh.triangles[0].v2, Vec3::new(10.0, 0.0, 0.0));
        assert!((mesh.max_dimension - 10.0).abs() < 1e-4);
    }

    #[test]
    fn test_binary_stl_roundtrip() {
        let mut bytes = vec![0u8; 84 + 50];
        // 1 triangle
        bytes[80] = 1;
        // Normal 0, 0, 1
        bytes[84 + 8] = 0x3f; // ~0.5.. or 1.0f32 le:
        let normal_z_bytes = 1.0f32.to_le_bytes();
        bytes[84 + 8..84 + 12].copy_from_slice(&normal_z_bytes);
        // v1 = 0, 0, 0
        // v2 = 20, 0, 0
        let v2_x = 20.0f32.to_le_bytes();
        bytes[84 + 24..84 + 28].copy_from_slice(&v2_x);
        // v3 = 0, 20, 0
        let v3_y = 20.0f32.to_le_bytes();
        bytes[84 + 40..84 + 44].copy_from_slice(&v3_y);

        let mesh = StlMesh::parse_bytes(&bytes, "bin_model".to_string()).unwrap();
        assert_eq!(mesh.triangles.len(), 1);
        assert!((mesh.max_dimension - 20.0).abs() < 1e-4);
    }

    #[test]
    fn test_part9_stl() {
        let p = std::path::Path::new("/home/vega/Downloads/Telegram Desktop/Part9.STL");
        if p.exists() {
            let mesh = StlMesh::parse_file(p).unwrap();
            println!("Part9: name={}, tris={}, center={:?}, size={:?}, max_dim={}",
                mesh.name, mesh.triangles.len(), mesh.center, mesh.size, mesh.max_dimension);
            
            // Check projected coordinates
            let pitch = 0.4f32;
            let yaw = 0.6f32;
            let cx = 400.0f32;
            let cy = 300.0f32;
            let scale = 30.0f32;

            let sin_yaw = yaw.sin();
            let cos_yaw = yaw.cos();
            let sin_pitch = pitch.sin();
            let cos_pitch = pitch.cos();
            let center = mesh.center;

            let transform_v = |v: Vec3| -> (f32, f32) {
                let dx = v.x - center.x;
                let dy = v.y - center.y;
                let dz = v.z - center.z;

                let rx = dx * cos_yaw + dz * sin_yaw;
                let rz = -dx * sin_yaw + dz * cos_yaw;
                let ry = dy;

                let fy = ry * cos_pitch - rz * sin_pitch;
                (cx + rx * scale, cy - fy * scale)
            };

            let mut out_of_bounds = 0;
            let mut huge_edges = 0;
            for (i, t) in mesh.triangles.iter().enumerate() {
                let p1 = transform_v(t.v1);
                let p2 = transform_v(t.v2);
                let p3 = transform_v(t.v3);

                let d1 = ((p1.0 - p2.0).powi(2) + (p1.1 - p2.1).powi(2)).sqrt();
                let d2 = ((p2.0 - p3.0).powi(2) + (p2.1 - p3.1).powi(2)).sqrt();
                let d3 = ((p3.0 - p1.0).powi(2) + (p3.1 - p1.1).powi(2)).sqrt();

                if p1.0 < -1000.0 || p1.0 > 2000.0 || p1.1 < -1000.0 || p1.1 > 2000.0 {
                    out_of_bounds += 1;
                }
                if d1 > 200.0 || d2 > 200.0 || d3 > 200.0 {
                    huge_edges += 1;
                    if huge_edges <= 5 {
                        println!("Huge edge in tri {}: v1={:?}, v2={:?}, v3={:?}, len={:?}", i, t.v1, t.v2, t.v3, (d1, d2, d3));
                    }
                }
            }
            println!("Out of bounds: {}, Huge edges (>200px): {}", out_of_bounds, huge_edges);
        }
    }
}

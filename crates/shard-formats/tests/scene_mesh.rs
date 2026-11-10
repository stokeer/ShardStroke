use std::path::Path;

use shard_formats::Container;

#[test]
fn vehicle_materials_match_mesh_indices() {
    let p = &std::env::var("SHARD_CORPUS").map(|d| std::path::PathBuf::from(d).join("vehicle.spp")).unwrap_or_default();
    if !p.exists() {
        return;
    }
    let mut c = Container::open(p).unwrap();
    let s = c.scene().unwrap();
    // 468 before the scan became byte-aligned-agnostic (11 meshes sat at non-4-aligned offsets), 479 before the
    // reader skipped the extra UV / colour channels (62 meshes carry a second UV channel)
    assert_eq!(s.meshes.len(), 541);
    assert_eq!(s.materials.len(), 8);
    assert!(s.meshes.iter().all(|m| (m.material as usize) < s.materials.len()));
}

#[test]
fn board_project_has_five_meshes() {
    let p = &std::env::var("SHARD_CORPUS").map(|d| std::path::PathBuf::from(d).join("board.spp")).unwrap_or_default();
    if !p.exists() {
        return;
    }
    let mut c = Container::open(p).unwrap();
    let ms = c.scene_meshes().unwrap();
    assert_eq!(ms.len(), 5);
    let b = &ms[0];
    assert_eq!(b.positions.len(), 63);
    assert_eq!(b.indices.len(), 45 * 3);
    assert!(b.uvs.is_some() && b.normals.is_some());
    // board is a thin slab: z spans [-1, 1], y thin
    let (zmin, zmax) = b.positions.iter().fold((f32::MAX, f32::MIN), |a, v| (a.0.min(v[2]), a.1.max(v[2])));
    assert!((zmin + 1.0).abs() < 1e-3 && (zmax - 1.0).abs() < 1e-3);
}

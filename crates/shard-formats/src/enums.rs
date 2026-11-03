//! Enum tables: value stored in Painter files -> API name.
//! Source: Painter 12.1.0 probe (`tools/painter_probe`), see `docs/FORMATS.md` 7.5d.

pub const BLENDING_MODE: [&str; 32] = [
    "Disable", "Replace", "Normal", "Darken", "Multiply", "ColorBurn", "LinearBurn", "Lighten",
    "Screen", "ColorDodge", "LinearDodge", "Overlay", "SoftLight", "HardLight", "VividLight",
    "LinearLight", "PinLight", "Difference", "Exclusion", "Tint", "Saturation", "Color", "Value",
    "Divide", "InverseDivide", "Passthrough", "NormalMapCombine", "NormalMapDetail",
    "NormalMapInverseDetail", "Subtract", "InverseSubtract", "SignedAddition",
];

pub const PROJECTION_MODE: [&str; 8] =
    ["UV", "Planar", "Triplanar", "Spherical", "Cylindrical", "UVSetToUVSet", "Fill", "Warp"];

/// Channel bit index -> name (None = unknown bit).
pub fn channel_name(bit: u32) -> Option<String> {
    let n = match bit {
        0 => "BaseColor",
        1 => "Height",
        2 => "Specular",
        3 => "Opacity",
        4 => "Emissive",
        5 => "Displacement",
        6 => "Glossiness",
        7 => "Roughness",
        8 => "Anisotropylevel",
        9 => "Anisotropyangle",
        10 => "Transmissive",
        11 => "Reflection",
        12 => "Ior",
        13 => "Metallic",
        14 => "BaseWeight",
        15 => "BaseDiffuseRoughness",
        16 => "SpecularWeight",
        17 => "TransmissionWeight",
        19 => "CoatRoughnessAnisotropy",
        20 => "ThinFilmWeight",
        21 => "Tangent",
        22 => "Normal",
        24 => "Diffuse",
        25 => "Specularlevel",
        26 => "BlendingMask",
        27 => "Scattering",
        28 => "SheenOpacity",
        29 => "SheenRoughness",
        30 => "SheenColor",
        33 => "CoatTangent",
        40 => "CoatOpacity",
        41 => "CoatColor",
        42 => "CoatRoughness",
        43 => "CoatSpecularLevel",
        44 => "CoatNormal",
        45 => "ScatteringColor",
        46 => "Translucency",
        47 => "SpecularEdgeColor",
        48 => "AbsorptionColor",
        64..=95 => return Some(format!("User{}", bit - 64)),
        _ => return None,
    };
    Some(n.to_string())
}

pub fn blending_mode_name(v: i64) -> Option<&'static str> {
    usize::try_from(v).ok().and_then(|i| BLENDING_MODE.get(i).copied())
}

pub fn projection_name(v: i64) -> Option<&'static str> {
    usize::try_from(v).ok().and_then(|i| PROJECTION_MODE.get(i).copied())
}

/// Names of set bits in a little-endian bitset (up to 128 bits).
pub fn channels_from_mask(mask: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    for (i, byte) in mask.iter().enumerate() {
        for b in 0..8 {
            if byte & (1 << b) != 0 {
                let bit = (i * 8 + b) as u32;
                out.push(channel_name(bit).unwrap_or_else(|| format!("#{bit}")));
            }
        }
    }
    out
}

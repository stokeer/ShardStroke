"""Enum tables: value stored in Painter files -> Python API name.

Source: tools/painter_probe/enum_probe.py run in Painter 12.1.0 + tools/probe_decode.py (2026-10-10).
Blending value 2 (Normal) is inferred (the default is not written to the file) and confirmed by UI
screenshots of Board.spp (2=Norm, 4=Mul, 25=Pthr).
"""

BLENDING_MODE = {
    0: "Disable", 1: "Replace", 2: "Normal", 3: "Darken", 4: "Multiply", 5: "ColorBurn",
    6: "LinearBurn", 7: "Lighten", 8: "Screen", 9: "ColorDodge", 10: "LinearDodge", 11: "Overlay",
    12: "SoftLight", 13: "HardLight", 14: "VividLight", 15: "LinearLight", 16: "PinLight",
    17: "Difference", 18: "Exclusion", 19: "Tint", 20: "Saturation", 21: "Color", 22: "Value",
    23: "Divide", 24: "InverseDivide", 25: "Passthrough", 26: "NormalMapCombine",
    27: "NormalMapDetail", 28: "NormalMapInverseDetail", 29: "Subtract", 30: "InverseSubtract",
    31: "SignedAddition",
}

PROJECTION_MODE = {
    0: "UV", 1: "Planar", 2: "Triplanar", 3: "Spherical", 4: "Cylindrical",
    5: "UVSetToUVSet",   # inferred (probe could not create it in an MR template)
    6: "Fill", 7: "Warp",
}

# bit index in DataBlending/DataAction.channelTypes == DataChannel.type
CHANNEL_TYPE = {
    0: "BaseColor", 1: "Height", 2: "Specular", 3: "Opacity", 4: "Emissive", 5: "Displacement",
    6: "Glossiness", 7: "Roughness", 8: "Anisotropylevel", 9: "Anisotropyangle", 10: "Transmissive",
    11: "Reflection", 12: "Ior", 13: "Metallic", 14: "BaseWeight", 15: "BaseDiffuseRoughness",
    16: "SpecularWeight", 17: "TransmissionWeight", 19: "CoatRoughnessAnisotropy",
    20: "ThinFilmWeight", 21: "Tangent", 22: "Normal", 24: "Diffuse", 25: "Specularlevel",
    26: "BlendingMask", 27: "Scattering", 28: "SheenOpacity", 29: "SheenRoughness", 30: "SheenColor",
    33: "CoatTangent", 40: "CoatOpacity", 41: "CoatColor", 42: "CoatRoughness",
    43: "CoatSpecularLevel", 44: "CoatNormal", 45: "ScatteringColor", 46: "Translucency",
    47: "SpecularEdgeColor", 48: "AbsorptionColor",
    **{64 + i: f"User{i}" for i in range(32)},
}
# aliases (same bit): EmissionColor=Emissive, SpecularRoughness=Roughness, BaseMetalness=Metallic,
# SpecularRoughnessAnisotropy=Anisotropylevel, FuzzWeight=SheenOpacity, FuzzRoughness=SheenRoughness,
# FuzzColor=SheenColor, CoatWeight=CoatOpacity, SubsurfaceRadiusScale=Scattering,
# SubsurfaceColor=ScatteringColor, SubsurfaceWeight=Translucency, SpecularColor=SpecularEdgeColor,
# TransmissionColor=AbsorptionColor

FILTERING = {0: "Nearest?", 1: "Bilinear?", 2: "Bilinear|HQ"}   # 2 confirmed by UI


def mask_to_channels(v):
    """channelTypes value (int or 'raw:<hex>') -> list of channel names."""
    if isinstance(v, str) and v.startswith("raw:"):
        v = int.from_bytes(bytes.fromhex(v[4:]), "little")
    if not isinstance(v, int):
        return []
    return [CHANNEL_TYPE.get(i, f"#{i}") for i in range(128) if (v >> i) & 1]

# ShardStroke: feature status

Every feature, tool by tool, with its current status. The plan and its order are in the
[roadmap](ROADMAP.md); what changed in each version is on the
[releases page](https://github.com/stokeer/ShardStroke/releases).

## How to read

| Status | Meaning |
|---|---|
| `done` | Implemented and verified against reference projects |
| `partial` | Works, with known gaps; does not block the rest |
| `approx` | Works, but the result is a measured approximation, not yet identical |
| `read` | Read from files and shown; not editable yet |
| `planned` | Not started |
| `redesign` | Will be done differently from the reference, on purpose |

Codenames: **Umbra** (painting), **Penumbra** (procedural authoring), **Chimera** (the procedural engine).

## Summary

| Area | Total | done | partial | approx | read | planned | redesign |
|---|---|---|---|---|---|---|---|
| PRJ. Projects and files | 23 | 2 | 6 |  | 2 | 13 |  |
| TXS. Texture sets and channels | 23 | 4 | 6 | 1 |  | 12 |  |
| LYR. Layer stack | 30 | 6 | 10 | 1 |  | 13 |  |
| BLD. Blend modes | 10 | 6 | 1 |  |  | 3 |  |
| EFX. Effects | 25 | 7 | 5 | 6 | 1 | 4 | 2 |
| PRO. Projections | 11 | 1 | 2 |  |  | 8 |  |
| TLS. Painting tools | 22 |  | 3 | 1 |  | 18 |  |
| BRS. Brushes, alphas, stencils | 12 |  | 3 |  | 1 | 8 |  |
| SUB. Procedurals (Chimera) | 18 | 5 | 3 | 5 |  | 5 |  |
| RES. Resources and shelf | 13 | 3 | 3 |  |  | 7 |  |
| BAK. Mesh map baking | 19 | 1 |  |  |  | 18 |  |
| VPT. Viewport, camera and display | 21 | 1 | 9 |  |  | 11 |  |
| RND. Shaders and rendering | 11 | 2 |  |  |  | 9 |  |
| CLR. Color management | 6 |  | 1 |  |  | 5 |  |
| EXP. Export | 12 |  | 5 |  |  | 7 |  |
| UI. Interface | 20 | 1 | 16 |  |  | 3 |  |
| SCR. Scripting and plugins | 5 |  | 1 |  |  | 4 |  |
| PRF. Performance and engine | 7 | 3 | 2 |  |  | 2 |  |
| PLS. Beyond the reference | 16 |  | 4 |  |  | 10 | 2 |
| **Total** | **304** | **42** | **80** | **14** | **4** | **160** | **4** |

Coverage (done, partial, approx or read): 140/304 (46%). Only `done`: 42/304.

## PRJ. Projects and files

| ID | Feature | Status |
|---|---|---|
| PRJ-01 | Create a project from a mesh: FBX, OBJ, glTF/GLB, USD, Alembic | `partial` |
| PRJ-02 | New project window: mesh, template, resolution, normal map format, UV tiles, mesh map import | `planned` |
| PRJ-03 | Project templates | `read` |
| PRJ-04 | Open projects of the industry-standard texturing software: stack, meshes, resources, strokes, mesh maps | `partial` |
| PRJ-05 | Open its smart materials, smart masks and tool presets | `read` |
| PRJ-06 | Save (Ctrl+S), save as (Ctrl+Shift+S) to `.shard`; save a copy, as template, reduced | `partial` |
| PRJ-08 | Recent projects, sample project | `planned` |
| PRJ-09 | Autosave, backup on save, lock file | `partial` |
| PRJ-10 | Undo / redo (Ctrl+Z, Ctrl+Y) with action names | `done` |
| PRJ-11 | History panel (go back to a state) | `planned` |
| PRJ-12 | Log panel, export log | `planned` |
| PRJ-13 | Project configuration (edit mesh, normal map format, color management after creation) | `planned` |
| PRJ-14 | Reimport mesh (Ctrl+Shift+R), keep stroke positions | `planned` |
| PRJ-15 | Drag a mesh, project or resource onto the viewport | `planned` |
| PRJ-16 | Physical size (unit scale), physical UV scale | `planned` |
| PRJ-17 | Import cameras, camera selection | `planned` |
| PRJ-18 | Auto-unwrap: islands, orientation, margins, texel density, hard surface | `planned` |
| PRJ-19 | Import baked mesh maps when creating a project | `planned` |
| PRJ-20 | Normal map format (DirectX / OpenGL), per-fragment tangent space | `partial` |
| PRJ-21 | Project workflow (metal/rough, spec/gloss, UV tiles) | `planned` |
| PRJ-22 | Project metadata and UUID | `partial` |
| PRJ-23 | Remove unused resources, import resources | `planned` |
| PRJ-24 | Warning for projects saved by a newer version | `done` |

## TXS. Texture sets and channels

| ID | Feature | Status |
|---|---|---|
| TXS-01 | One texture set per mesh material, each with its own stack | `done` |
| TXS-02 | Texture set list: visibility, resolution, shader instance, solo / focus, multi-selection | `partial` |
| TXS-03 | Resolution per set (256 to 8K) | `partial` |
| TXS-04 | Non-square and per-channel resolution | `planned` |
| TXS-05 | Bit depth per channel (8, 16, 32) | `planned` |
| TXS-06 | Base color channel | `done` |
| TXS-07 | Roughness channel | `done` |
| TXS-08 | Metallic channel | `done` |
| TXS-09 | Normal channel: base normal map + layers, mixing (replace / combine) | `approx` |
| TXS-10 | Height channel (and displacement) | `partial` |
| TXS-11 | Opacity channel | `planned` |
| TXS-12 | Emissive channel | `partial` |
| TXS-13 | Ambient occlusion channel, replace / multiply mixing | `partial` |
| TXS-14 | Advanced material channels: coat, sheen, transmission, subsurface, anisotropy, thin film... | `planned` |
| TXS-15 | Spec/gloss workflow: diffuse, specular, glossiness | `planned` |
| TXS-16 | User channels (32, RGBA) | `planned` |
| TXS-17 | Manage channels window (add / remove, apply to all sets) | `planned` |
| TXS-18 | Texture set settings: normal mixing, height to normal, AO mixing, UV padding | `planned` |
| TXS-19 | UV tiles (UDIM): paint across tiles, mask per tile, resolution per tile | `planned` |
| TXS-20 | Paint several materials at once | `partial` |
| TXS-21 | Material layering (up to 10 materials) | `planned` |
| TXS-22 | Shader instance per texture set | `planned` |
| TXS-23 | Isolate texture set (Alt+Q), show only the selected set while painting | `planned` |

## LYR. Layer stack

| ID | Feature | Status |
|---|---|---|
| LYR-01 | Layer tree and nested groups | `done` |
| LYR-02 | Paint layer (hand painting stored as strokes) | `approx` |
| LYR-03 | Fill layer: material mode or a source per channel | `partial` |
| LYR-04 | Group layer, passthrough mode | `done` |
| LYR-05 | Visibility (eye), Shift+click on the mask | `done` |
| LYR-06 | Opacity per channel | `done` |
| LYR-07 | Blend mode per channel (see BLD) | `done` |
| LYR-08 | Layer / group mask, black or white background; context menu (invert background...) | `partial` |
| LYR-09 | Mask effect stack (fill, paint, filter, levels, anchor, polygon fill) | `partial` |
| LYR-10 | Layer content effect stack | `partial` |
| LYR-11 | Add paint / fill / group layer, group (Ctrl+G), ungroup, delete (Del) | `partial` |
| LYR-12 | Reorder by drag, move between groups | `done` |
| LYR-13 | Multi-selection of layers and effects | `planned` |
| LYR-14 | Duplicate (Ctrl+D), copy / cut / paste (Ctrl+C/X/V), paste between texture sets | `partial` |
| LYR-15 | Instance layers (paste as instance, across texture sets, jump to source) | `planned` |
| LYR-16 | Smart materials: apply from the shelf, create, save | `planned` |
| LYR-17 | Smart masks: apply, create, save | `planned` |
| LYR-18 | Geometry mask by mesh name or UV tile, include / exclude (Alt+H) | `planned` |
| LYR-19 | Add mask: white, black, bitmap, with color selection, with height combination | `partial` |
| LYR-20 | Mask actions: enable / disable, invert, invert background, clear, remove | `partial` |
| LYR-21 | Export mask (file, clipboard), export layer channels | `planned` |
| LYR-22 | Flatten layer / group (Ctrl+M), flatten instances | `planned` |
| LYR-23 | Layer color tag (7 colors) | `planned` |
| LYR-24 | Gamma-corrected alpha / mask indicator | `planned` |
| LYR-25 | Blending options: copy / paste, apply to all channels | `planned` |
| LYR-26 | Channel filter of the layers panel (dropdown, Alt+click) | `planned` |
| LYR-27 | Layer and mask thumbnails (real content of fill layers and masks; paint layers, filters and groups to come) | `partial` |
| LYR-28 | Copy / paste effects, drag an effect between layers, insert above | `planned` |
| LYR-29 | Rename layer (double click or menu) and effect | `partial` |
| LYR-30 | Pasting a layer creates its mask; paste into the mask | `planned` |

## BLD. Blend modes

| ID | Feature | Status |
|---|---|---|
| BLD-01 | Normal, Replace, PassThrough | `done` |
| BLD-02 | Multiply, Divide, InverseDivide | `done` |
| BLD-03 | Darken, Lighten | `done` |
| BLD-04 | LinearDodge (Add), Subtract, InverseSubtract, Difference, Exclusion | `done` |
| BLD-05 | Screen, Overlay, SoftLight, HardLight | `done` |
| BLD-06 | LinearBurn, ColorBurn, ColorDodge, VividLight, LinearLight, PinLight | `done` |
| BLD-07 | Disable | `planned` |
| BLD-08 | SignedAddition | `planned` |
| BLD-09 | Tint, Saturation, Color, Value (non-separable, color only) | `planned` |
| BLD-10 | NormalMapCombine, NormalMapDetail, NormalMapInverseDetail | `partial` |

## EFX. Effects

| ID | Feature | Status |
|---|---|---|
| EFX-01 | Fill: uniform color / value source | `done` |
| EFX-02 | Fill: bitmap source (project texture, UV projection) | `done` |
| EFX-03 | Fill: procedural source | `approx` |
| EFX-04 | Fill: reference source (anchor point) | `done` |
| EFX-05 | Fill: font / text and vector sources (SVG) | `planned` |
| EFX-06 | Fill: UV transformations (tiling, rotation, offset, wrap, scale mode) | `done` |
| EFX-07 | Fill: filtering (nearest, bilinear, bilinear HQ) | `partial` |
| EFX-08 | Fill: projections beyond UV | `planned` |
| EFX-09 | Fill: symmetry | `planned` |
| EFX-10 | Procedural source: resolution override and factor | `done` |
| EFX-11 | Procedural source: color space per output | `partial` |
| EFX-12 | Paint effect (hand painting in an effect) | `approx` |
| EFX-13 | Eraser effect | `approx` |
| EFX-14 | Smudge effect | `read` |
| EFX-15 | Generator effect (mask generators of the shelf) | `approx` |
| EFX-16 | Filter effect (shelf filters, blend and opacity per filter); native and measured: sharpen, highpass, contrast/luminosity | `approx` |
| EFX-17 | Levels (mono and RGB, histogram, auto, invert, clamp) | `partial` |
| EFX-18 | Color selection (by ID map color) | `redesign` |
| EFX-19 | Compare mask | `redesign` |
| EFX-20 | Anchor point (exposes a point of the stack) | `done` |
| EFX-21 | Polygon fill / geometry selection (triangle, polygon, element, UV island) | `partial` |
| EFX-22 | Add effect menu (generator, paint, fill, levels, compare mask, filter, color selection, anchor) | `planned` |
| EFX-23 | Universal properties per effect (fill / generator / filter) | `partial` |
| EFX-24 | Fixed effects of the shelf (gradient, histogram, blur...) as filters | `approx` |
| EFX-25 | Blending between the content effects of a layer (mode and opacity per channel) | `done` |

## PRO. Projections

| ID | Feature | Status |
|---|---|---|
| PRO-01 | UV projection | `done` |
| PRO-02 | Planar | `partial` |
| PRO-03 | Tri-planar (parameters, manipulators) | `partial` |
| PRO-04 | Spherical | `planned` |
| PRO-05 | Cylindrical (cap removal) | `planned` |
| PRO-06 | UV set to UV set | `planned` |
| PRO-07 | Fill (mesh data, match per UV tile) | `planned` |
| PRO-08 | Warp (grid, decal, warp to geometry) | `planned` |
| PRO-09 | Shape crop, depth and backface culling | `planned` |
| PRO-10 | Projection manipulators (Q, W, E, R, T, surface tool) | `planned` |
| PRO-11 | Physical projection by collision (3D plane in the scene) | `planned` |

## TLS. Painting tools

| ID | Feature | Status |
|---|---|---|
| TLS-01 | Paint (1): brush, size, flow, opacity, spacing, hardness | `partial` |
| TLS-02 | Paint all channels at once (color, height, roughness, metallic, normal, opacity, emissive) | `planned` |
| TLS-03 | Eraser (2) with channel selection | `partial` |
| TLS-04 | Projection (3): tiling mode, grayscale, texture | `planned` |
| TLS-05 | Polygon fill (4): triangle, polygon, element, UV island | `planned` |
| TLS-06 | Smudge (5) | `planned` |
| TLS-07 | Clone (6 relative, Ctrl+6 absolute, V+LMB source) | `planned` |
| TLS-08 | Material picker (P), color picker (I), color-managed picker | `planned` |
| TLS-09 | Physical paint / eraser / projection (Ctrl+1, 2, 3) | `planned` |
| TLS-10 | Paint, erase, smudge, filled and ribbon along a path | `planned` |
| TLS-11 | Quick mask (Y, U, Ctrl+Shift+I) | `planned` |
| TLS-12 | Mirror and radial symmetry (L), offset, panel | `planned` |
| TLS-13 | Lazy mouse (D), straight lines (Shift+LMB), resume stroke (A) | `planned` |
| TLS-14 | Pen / tablet: pressure on size and flow, curves, minimums | `planned` |
| TLS-15 | Size / hardness / flow / rotation with the mouse (Ctrl+RMB / Ctrl+LMB), `[` `]` | `partial` |
| TLS-16 | Invert grayscale (X), ignore stencil (N) | `planned` |
| TLS-17 | Paint in a mask (automatic tool switch, brush color follows the mask) | `planned` |
| TLS-18 | Replay of the strokes of imported projects | `approx` |
| TLS-19 | Pause the engine (Shift+Esc) | `planned` |
| TLS-20 | Skew correction painting (low poly in the bake) | `planned` |
| TLS-21 | Texture set selection by click (Ctrl+Alt+RMB), warning when painting another set | `planned` |
| TLS-22 | Select UV chunks / elements to paint | `planned` |

## BRS. Brushes, alphas, stencils

| ID | Feature | Status |
|---|---|---|
| BRS-01 | Brush parameters: size, flow, stroke opacity, spacing, hardness, angle, jitter | `partial` |
| BRS-02 | Brush projection: alignment (tangent / wrap), backface culling, size space | `planned` |
| BRS-03 | Alphas: attributes, color spaces, resolution, parameters | `planned` |
| BRS-04 | Stencils (S+LMB), tiling mode, display opacity | `partial` |
| BRS-05 | Brush / tool presets (276 in the shelf) | `read` |
| BRS-06 | Import ABR brushes | `planned` |
| BRS-07 | Dynamic strokes (stamp index, time, random seed) | `planned` |
| BRS-08 | Particles: emitters and receivers | `planned` |
| BRS-09 | Brush material: channels, material mode, source per channel | `planned` |
| BRS-10 | Stroke and sphere preview in the properties panel | `planned` |
| BRS-11 | Text / font in brushes and fills, advanced properties | `partial` |
| BRS-12 | Brush preview: full, outline, crosshair | `planned` |

## SUB. Procedurals (Chimera)

| ID | Feature | Status |
|---|---|---|
| SUB-01 | Read procedural material interfaces (parameters, groups, visibility, combos, defaults, outputs) | `done` |
| SUB-02 | Graph parameter UI (slider, toggle, combo, color, seed, restore defaults) | `done` |
| SUB-03 | Chimera: run compiled procedural graphs natively (reader done for every graph of the corpus; levels, blend, directional blur, sharpen, uniform color, image inputs) | `partial` |
| SUB-04 | Noise generators (clouds, perlin, spots, moisture, anisotropic...) | `approx` |
| SUB-05 | Grunges, dirt and scratches | `approx` |
| SUB-06 | Metal edge wear mask generator | `approx` |
| SUB-07 | Curvature, ambient occlusion, dirt and mask editor generators | `approx` |
| SUB-08 | Dripping rust, fiber glass edge wear, mask builder, position, world space normal generators | `planned` |
| SUB-09 | General generators: 3D distance, 3D linear gradient, advanced tri-planar, grayscale conversion... | `planned` |
| SUB-10 | Filters: blur, sharpen, highpass, contrast/luminosity, perceptual HSL, linear gradient | `approx` |
| SUB-11 | Filters: bevel, directional / slope blur, clamp, color balance / correct / match, drop shadow... | `planned` |
| SUB-12 | Finishes and material effects (brushed, galvanized, rust weathering, peeling paint, water drops...) | `planned` |
| SUB-13 | Resolution override, seed, tweaks per source | `done` |
| SUB-14 | Graph image inputs (chaining) and mesh map inputs (AO, curvature, position...) | `partial` |
| SUB-15 | Procedural materials (base color / roughness / metallic / normal / height outputs) | `partial` |
| SUB-16 | Export textures as a procedural package | `planned` |
| SUB-17 | Exact render through an installed external renderer (optional, disk cache) | `done` |
| SUB-18 | Bitmaps embedded in procedural graphs | `done` |

## RES. Resources and shelf

| ID | Feature | Status |
|---|---|---|
| RES-01 | Libraries (user, starter, fonts), default, add / remove | `done` |
| RES-02 | Resource types: alphas, color LUTs, effects, emitters, environments, export presets, fonts... | `partial` |
| RES-03 | Assets panel: search, type filters, grid / list, thumbnails | `partial` |
| RES-04 | Slot resource picker (search, filters, grid / list, details, anchor points) | `done` |
| RES-05 | Embedded previews | `done` |
| RES-06 | Drag a resource to the stack / viewport (materials, filters, smart masks, decals) | `planned` |
| RES-07 | Import resources, choose the target folder | `planned` |
| RES-08 | Favorites, rename, reload, delete, tags | `planned` |
| RES-09 | Auto-update of modified resources | `planned` |
| RES-10 | Shelf API: crawling, events, project resources | `planned` |
| RES-12 | Import SVG as a resource | `planned` |
| RES-13 | Import UDIM sequences as a resource | `planned` |
| RES-14 | Color LUTs and environments | `partial` |

## BAK. Mesh map baking

| ID | Feature | Status |
|---|---|---|
| BAK-01 | Read the baked mesh maps of a project (normal, AO, curvature, position, ID...) | `done` |
| BAK-02 | Normal baker | `planned` |
| BAK-03 | World space normal baker | `planned` |
| BAK-04 | ID baker (color / material / mesh) | `planned` |
| BAK-05 | Ambient occlusion baker (rays, distances, spread, ground plane...) | `planned` |
| BAK-06 | Curvature baker (from mesh / from normal map) | `planned` |
| BAK-07 | Position baker | `planned` |
| BAK-08 | Thickness baker | `planned` |
| BAK-09 | Height baker | `planned` |
| BAK-10 | Bent normals baker | `planned` |
| BAK-11 | Opacity baker | `planned` |
| BAK-12 | Common settings: size, anti-aliasing, dilation, high poly, match by name, suffixes | `planned` |
| BAK-13 | Cage: distance, front / rear, relative to bounding box, auto cage | `planned` |
| BAK-14 | Skew correction: painting, edge protection, auto re-bake | `planned` |
| BAK-15 | Bake mode: split 3D / 2D, list of bakers, log, preview | `planned` |
| BAK-16 | Baking per UV tile / selection, sync between texture sets, link groups | `planned` |
| BAK-17 | GPU ray tracing | `planned` |
| BAK-18 | High-poly cache, save the preprocessed scene | `planned` |
| BAK-19 | Continuous re-bake per map | `planned` |

## VPT. Viewport, camera and display

| ID | Feature | Status |
|---|---|---|
| VPT-01 | 3D viewport: orbit (Alt+LMB), pan (Alt+MMB), zoom (Alt+RMB / wheel), show entire mesh (F) | `done` |
| VPT-02 | Display modes: channel (C), material (M), mesh map (B) | `partial` |
| VPT-03 | 2D (UV) viewport: 3D / 2D layouts (F1 to F4), painting in 2D | `planned` |
| VPT-04 | Camera: FOV, focal length, orthographic (F5, F6), focus distance, aperture, presets | `planned` |
| VPT-05 | Project cameras and navigation | `planned` |
| VPT-06 | Environment: HDRI, rotation, exposure, blur, opacity, alignment | `partial` |
| VPT-07 | Shadows | `planned` |
| VPT-08 | Tone mapping, color profile (LUT), color correction | `partial` |
| VPT-09 | Anti-aliasing (MSAA 4x done; temporal and accumulation to come) | `partial` |
| VPT-10 | Subsurface scattering in the viewport | `planned` |
| VPT-11 | Post effects: depth of field, bloom, glare, lens flare, chromatic aberration, vignette, sharpen, film grain | `partial` |
| VPT-12 | Wireframe, grid, UV seams, camera frame overlay | `planned` |
| VPT-13 | Display settings window | `partial` |
| VPT-14 | Texture filtering (mipmaps, anisotropic, mip bias), quality, virtual textures | `partial` |
| VPT-15 | Solo channel view (R, G, B, A), HDR, grayscale | `planned` |
| VPT-16 | Axis gizmo, circular brush cursor, 3D brush preview | `partial` |
| VPT-17 | Viewport screenshot | `partial` |
| VPT-18 | 3D mouse support | `planned` |
| VPT-19 | Detach 2D / 3D viewports (multi-monitor) | `planned` |
| VPT-20 | Hide UI (Tab) | `planned` |
| VPT-21 | Displacement and tessellation in the viewport | `planned` |

## RND. Shaders and rendering

| ID | Feature | Status |
|---|---|---|
| RND-01 | PBR metal / roughness shader (split-sum IBL, GGX, ACES, stored TBN) | `done` |
| RND-02 | Metal / roughness shader with alpha blend and alpha test | `planned` |
| RND-03 | Spec / gloss shaders | `planned` |
| RND-04 | Coated, car paint, velvet, anisotropy, clay shaders | `planned` |
| RND-05 | OpenPBR shader (anisotropy, coat, SSS, sheen) | `planned` |
| RND-06 | Game engine shaders | `planned` |
| RND-07 | Shader settings: parameters per shader, restore defaults | `planned` |
| RND-08 | Baked AO applied to indirect light | `done` |
| RND-09 | Parallax occlusion mapping, bent normals in shading | `planned` |
| RND-11 | Import custom shaders (GLSL) | `planned` |
| RND-12 | Neutral material in bake previews | `planned` |

## CLR. Color management

| ID | Feature | Status |
|---|---|---|
| CLR-01 | Color space per channel (sRGB, raw, normal, data, signed data) | `partial` |
| CLR-02 | OpenColorIO (ACES), ACEScg working space | `planned` |
| CLR-03 | Bitmap import defaults (8, 16, float), auto-detect by name | `planned` |
| CLR-04 | Export color pipeline (space per type / depth) | `planned` |
| CLR-05 | Color-managed picker, swatches | `planned` |
| CLR-06 | ICC profiles | `planned` |

## EXP. Export

| ID | Feature | Status |
|---|---|---|
| EXP-01 | Export packed texture sets with dilation | `partial` |
| EXP-02 | Export textures window: settings, output templates, list of exports | `partial` |
| EXP-03 | Export presets and custom templates, name tokens | `partial` |
| EXP-04 | Formats: PNG, TGA, JPG, TIFF, EXR (16 and 32 bit), PSD (groups) | `partial` |
| EXP-05 | Bit depth, dithering, padding (dilation / diffusion / none), size per set | `partial` |
| EXP-06 | Converted maps: 1/IOR, mixed AO, diffuse, f0, glossiness, DirectX / OpenGL normal, reflection... | `planned` |
| EXP-07 | Export per UV tile (UDIM) | `planned` |
| EXP-08 | Mesh export (FBX, OBJ, glTF/GLB, USD, USDZ), displacement, triangulation | `planned` |
| EXP-09 | Export shader parameters (JSON), USD with shaders | `planned` |
| EXP-11 | Background export with progress, cancel | `planned` |
| EXP-12 | Export a mask / layer / channels to a file | `planned` |
| EXP-13 | OpenPBR naming convention | `planned` |

## UI. Interface

| ID | Feature | Status |
|---|---|---|
| UI-01 | Main layout: menu bar, toolbar, docks (assets / properties, viewport, layers / texture sets) | `partial` |
| UI-02 | Menus: File, Edit, Mode, Window, Viewport, Help | `partial` |
| UI-03 | Universal PROPERTIES panel (follows the last tool / layer / effect) | `partial` |
| UI-04 | Properties: paint (brush, alpha, stencil, material) | `partial` |
| UI-05 | Properties: fill (attributes, color spaces, resolution, parameters) | `partial` |
| UI-06 | Properties: generator / filter / levels / anchor point | `partial` |
| UI-07 | Full layers panel (toolbar, context menu, masks, child effects) | `partial` |
| UI-08 | Texture set settings (general, channels, mesh maps) | `planned` |
| UI-09 | Display settings panel | `partial` |
| UI-10 | Shader settings panel | `partial` |
| UI-11 | Settings: general, shortcuts (editable), libraries | `partial` |
| UI-12 | Window > views, toolbars, save / load / reset layout, tabbed docks, floating panels | `partial` |
| UI-13 | Industry-standard shortcuts | `partial` |
| UI-14 | Status bar: messages, progress of long tasks, cache | `partial` |
| UI-15 | Modern monochrome dark theme | `done` |
| UI-16 | Sliders: double click to edit, Shift for precision, arrows | `partial` |
| UI-17 | Tooltips, keyboard helper, warnings | `partial` |
| UI-18 | Localization | `planned` |
| UI-20 | Tool strip with groups and flyouts | `partial` |
| UI-21 | Modern welcome screen: recent projects with thumbnails, new project from presets | `planned` |

## SCR. Scripting and plugins

| ID | Feature | Status |
|---|---|---|
| SCR-01 | Python API: project, layer stack, export, baking, display, resources | `planned` |
| SCR-02 | JavaScript and QML plugin UI | `planned` |
| SCR-03 | Plugins (start / close / reload), built-in plugins | `planned` |
| SCR-04 | Events (project, export, baking, shelf, layer stack, display) | `planned` |
| SCR-05 | Command line (open, batch export) | `partial` |

## PRF. Performance and engine

| ID | Feature | Status |
|---|---|---|
| PRF-01 | Per-set textures on demand, streamed uploads (lean RAM and VRAM) | `done` |
| PRF-02 | Asynchronous mask previews at 3 resolutions, per-layer cache | `done` |
| PRF-03 | GPU compositor with an incremental per-node cache | `partial` |
| PRF-04 | Parallel stroke computation | `planned` |
| PRF-05 | VRAM budget, block compression, virtual textures | `planned` |
| PRF-06 | Automatic fidelity measurement against reference projects (`shard-measure`, baseline) | `done` |
| PRF-07 | Pause engine, computation indicator, viewport statistics | `partial` |

## PLS. Beyond the reference

| ID | Feature | Status |
|---|---|---|
| PLS-01 | Path tracer in the viewport (wgpu compute, AI denoiser) | `partial` |
| PLS-02 | Render-quality real-time viewport: GTAO, soft shadows, contact shadows, SSR | `planned` |
| PLS-03 | Incremental GPU compositor: re-evaluates only what changed, instant mask previews | `planned` |
| PLS-04 | Better color / ID selection (several colors, tolerance, feather, per island / mesh / material) | `redesign` |
| PLS-05 | Mask logic instead of compare mask (operations between channels and masks with preview) | `redesign` |
| PLS-07 | Project snapshots / versions (go back, compare layers between versions) | `planned` |
| PLS-08 | Command palette (Ctrl+K) and global search of resources, layers and settings | `planned` |
| PLS-09 | Live preview of any shelf resource (procedurals rendered instead of glyphs) | `planned` |
| PLS-10 | Native procedural authoring (Penumbra node editor) | `planned` |
| PLS-11 | Seam-aware painting, diffusion padding | `planned` |
| PLS-12 | Live link: reload exported textures in the game engine | `planned` |
| PLS-13 | Built-in fidelity diff against exported textures | `planned` |
| PLS-15 | Upscalers in the viewport (FSR, DLSS, XeSS) | `planned` |
| PLS-16 | Two viewport render modes: real time and full quality (accumulated path tracing) | `partial` |
| PLS-17 | Quick panel on right click in the viewport | `partial` |
| PLS-18 | New UI kit (validated clickable mockup being ported) | `partial` |

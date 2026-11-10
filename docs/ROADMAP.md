# Roadmap

ShardStroke is built from the bottom up: data → compositor → painting → viewport → bake → export → resources →
UI. A feature starts only when the layer it stands on is solid, and "done" means measured against the result of
the original project, not "looks similar".

## Where it stands (0.1.0 preview)

| Area | State |
|---|---|
| Opening projects of the industry-standard texturing software, its materials, OBJ meshes | Works: layer stack, masks, fills, effects and the saved texture sets |
| Layer compositor (blend modes, masks, fills, levels) | Most blend modes and effects done; height, opacity and user channels in progress |
| Procedurals (generators, filters) | Native approximations; exact results when a material authoring tool is installed (optional) |
| Painting | Texture-space brush with the industry-standard shortcuts; multi-channel painting, symmetry and tablet still to come |
| Projections | UV fill done; planar, tri-planar, spherical and cylindrical in progress |
| Viewport | Real-time PBR (HDRI, bloom, tone mapping, MSAA) and a path tracer |
| Bake (mesh maps) | Reads the maps saved in the project; own bakers still to come |
| Export | Templates (Arma Reforger BCR/NMO, PBR Metallic Roughness, Unreal, Unity HDRP, glTF); PNG, TGA, JPEG, TIFF, BMP, EXR |
| Interface | Familiar layout with dockable panels, History, Log, Display and Shader settings, quick panel |
| Own project format | `.shard` save/open, autosave backups |

## Next, in order

1. **Editable, measured base**: GPU compositor that re-evaluates only what changed (instant previews), the
   remaining blend modes and channels, resolution and colour management.
2. **Faithful content**: non-UV projections, geometry masks, padding, smart materials and masks, instance layers,
   a native procedural engine.
3. **Real painting**: multi-channel painting, a faithful brush engine, the full tool set, painting on masks,
   symmetry, lazy mouse, quick mask, tablet pressure, brush presets.
4. **See and produce**: 2D UV view, display settings, shaders, shadows and ambient occlusion, mesh map bakers,
   FBX / glTF / USD import, UDIM.
5. **Complete shell**: full Layers panel, Texture Set Settings, layouts, New Project, welcome screen.

## Beyond the standard

| Idea | State |
|---|---|
| Progressive path tracer in the viewport (GPU BVH, environment importance sampling, edge-aware denoiser, Open Image Denoise when the view is still) | Working; OpenPBR materials and depth of field to come |
| Two render buttons in the viewport: real-time and full quality | Done |
| Quick panel on right click (colour, brush, recent colours and brushes), pinnable | Done |
| Viewport upscalers (FSR, DLSS, XeSS) | Planned |
| Real-time quality: GTAO, soft and contact shadows, SSR, area lights | Planned |
| Seam-aware painting and diffusion padding | Planned |
| Better colour/ID selection, mask logic operations | Planned |
| Project snapshots and versions, command palette (Ctrl+K) | Planned |
| Live preview of every resource in the shelf | Planned |
| Native node editor for procedurals | Planned |
| Live link: reload exported textures in the game engine | Planned |

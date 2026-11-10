<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/brand/logo-horizontal-white.svg">
    <img src="docs/brand/logo-horizontal.svg" alt="ShardStroke" width="420">
  </picture>
</p>

<p align="center">
  <b>Create materials procedurally and paint them in 3D in the same environment, without switching tools.</b>
</p>

<p align="center">
  <a href="https://github.com/stokeer/ShardStroke/releases">Download</a> ·
  <a href="docs/STATUS.md">Feature status</a> ·
  <a href="docs/ROADMAP.md">Roadmap</a> ·
  <a href="LICENSE">Apache 2.0</a>
</p>

---

ShardStroke is an open-source 3D PBR texture painter written in Rust and wgpu. It opens projects made in the
industry-standard texturing software and reproduces their results, measured layer by layer against their exported
textures, and then goes further: a path-traced viewport, a native procedural engine and a faster, more fluid
workflow.

> **Early preview (alpha).** Expect missing features and rough edges. Windows is the tested platform.

## Highlights

- **Opens existing projects**: layer stacks, masks, fills, generators, filters, anchor points and painted strokes.
- **Measured fidelity**: every channel is compared against the reference exports of real projects.
- **Path-traced viewport** with an AI denoiser that refines while the view is still.
- **Chimera**, a native procedural engine that runs compiled procedural graphs without an external renderer
  (in progress).
- **Instant previews**: incremental compositing with per-layer caches, and a GPU compositor in validation.
- **Familiar controls**: industry-standard shortcuts, so existing muscle memory works.

## Projects

| Codename | Area |
|---|---|
| **Umbra** | Painting: layer stack, brushes, masks, viewport |
| **Penumbra** | Procedural material authoring |
| **Chimera** | The procedural engine behind both |

See [feature status](docs/STATUS.md) for what works today, tool by tool.

## Download

Windows builds are on the [releases page](https://github.com/stokeer/ShardStroke/releases):

- **Numbered previews**: tested snapshots.
- **`test` pre-release**: rebuilt automatically from every change to `main` (newest, may be broken).

## Building from source

Requirements: a recent stable Rust toolchain and a GPU with Vulkan, DirectX 12 or Metal support.

```
cargo build --release
cargo run --release -p shard-app -- [--obj MESH.obj] [--project FILE]
```

The optional AI denoiser needs the Open Image Denoise DLLs in `third_party/oidn/bin/`
(see [`third_party/oidn/README.md`](third_party/oidn/README.md)); release packages include them.

### Controls

| Action | Input |
|---|---|
| Paint | LMB |
| Orbit / zoom / pan | Alt+LMB / Alt+RMB or wheel / Alt+MMB |
| Brush size | `[` `]` |
| Paint / eraser | `1` / `2` |
| Next / previous channel | `C` / `Shift+C` |
| Mesh map view / material view | `B` / `M` |
| Frame mesh | `F` |
| Export textures | `Ctrl+Shift+E` |

Headless render for visual checks: `shard-app --shot out.png --size 960x540 --project FILE`.

## Development

```
cargo test --release                     # corpus tests run when SHARD_CORPUS is set
target/release/shard-dump FILE           # list the members of a project file
target/release/shard-dump FILE member    # decode a member to JSON
```

`SHARD_CORPUS` points to a folder of test projects; without it the corpus tests are skipped. Test projects and
their exports are not part of the repository.

### Code layout

| Crate | Purpose |
|---|---|
| `shard-formats` | File readers: project containers, material interfaces, compiled procedural graphs, mesh scenes |
| `shard-model` | Layer-stack model and project importer |
| `shard-compose` | CPU reference compositor and the Chimera graph interpreter (ground truth for the GPU path) |
| `shard-gpu` | GPU compute compositing, checked against the CPU compositor |
| `shard-engine` | Headless engine: project loading, mesh, strokes, bitmaps, composition, `shard-measure` |
| `shard-app` | Application: wgpu viewport, texture-space painting, UI |

### Principles

- **Foundation first.** A feature starts only when the layer it stands on is solid.
- **Measured, not "looks similar".** Behaviour is matched layer by layer, mask by mask, against reference exports.
- **Fast and fluid.** Every preview (masks, generators, filters) must respond instantly.

## Interoperability and licensing

- ShardStroke reads the **data files** users already own (projects, materials, presets) for interoperability.
  No third-party program, engine or library is used, modified or redistributed.
- No third-party assets or project data are part of this repository.
- When a material authoring tool with a command-line renderer is installed, it can be used as an optional external
  tool. It is never bundled.
- ShardStroke is an independent project, not affiliated with any other software vendor. See [`NOTICE`](NOTICE).

Licensed under the [Apache License 2.0](LICENSE). Bundled open-source components and their licences are listed in
[`NOTICE`](NOTICE).

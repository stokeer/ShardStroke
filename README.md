<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/brand/logo-horizontal-white.svg">
    <img src="docs/brand/logo-horizontal.svg" alt="ShardStroke" width="420">
  </picture>
</p>

# ShardStroke

Open-source 3D PBR texture painter written in Rust + wgpu. First goal: **open projects made in the
industry-standard texturing software** and reproduce their results, measured layer by layer. Second goal: **go
beyond it**: a path-traced viewport (**Umbra**, the project's own renderer), a faster engine and
quality-of-life improvements. See the [roadmap](docs/ROADMAP.md); what changed in each version is on the
[releases page](https://github.com/stokeer/ShardStroke/releases).

> **Status: early preview (alpha).** Expect missing features and rough edges. Windows is the tested platform.

> ShardStroke is an independent project, not affiliated with any other software vendor. See [`NOTICE`](NOTICE).

## Download

Windows builds are on the [releases page](https://github.com/stokeer/ShardStroke/releases): the numbered
previews, and the `test` pre-release, rebuilt automatically from every change to `main` (newest, may be broken).

## Code layout

| Path | What |
|---|---|
| `crates/shard-formats` | File readers: project containers, material interfaces, mesh scenes, `shard-dump` CLI |
| `crates/shard-model` | Layer-stack model and project importer |
| `crates/shard-compose` | CPU reference compositor: per-channel blend, masks, fills, procedurals (ground truth for the GPU path) |
| `crates/shard-engine` | Headless engine (no window, no GPU): project loading, mesh, strokes, bitmaps, composition, `shard-measure` |
| `crates/shard-app` | wgpu viewport, texture-space painting, UI (winit + egui; `--shot` headless render) |
| `tools/` | Python exploration scripts (reference implementation, golden-data generators) |

## Run the app

```
cargo run --release -p shard-app -- [--obj MESH.obj] [--project FILE]
```

LMB paint, Alt+LMB orbit, Alt+RMB / wheel zoom, Alt+MMB pan, `[` `]` brush size, `1` paint, `2` eraser,
`C` / `Shift+C` channel, `B` / `Shift+B` mesh map, `M` material, `F` frame mesh, Ctrl+Shift+E export. The
shortcuts follow the industry-standard ones, so existing muscle memory works.

Headless render (used for visual checks): `shard-app --shot out.png --size 960x540 --project FILE`.

## Build / test

```
cargo build --release
cargo test --release                     # corpus tests run when SHARD_CORPUS is set (see below)
target/release/shard-dump FILE           # list the members of a project file
target/release/shard-dump FILE member    # decode a member to JSON
```

`SHARD_CORPUS` is a folder of test projects (`board`, `screen` and `vehicle`); without it the corpus tests are
skipped. Test projects and their exports are not part of the repository.

The optional AI denoiser needs the Open Image Denoise DLLs in `third_party/oidn/bin/` (see
[`third_party/oidn/README.md`](third_party/oidn/README.md)); the release packages include them.

## How we work

- **Foundation first.** A feature is only started when the layer it stands on is solid (see the
  [roadmap](docs/ROADMAP.md)). No half features built on a missing base.
- **Layer by layer, measured.** Behaviour is matched against the results of the original projects, mask by mask,
  layer by layer. "Done" means measured, not "looks similar".
- **Fast and fluid.** Every preview (masks, generators, filters) must respond instantly; measure with
  `SHARD_TRACE`.

## Rules

- Interoperability only: ShardStroke reads the **data files** users already own (projects, materials, presets).
  No third-party program, engine or library is used, modified or redistributed.
- No third-party assets in this repository (materials, HDRIs and textures stay on the user's machine).
- When a material authoring tool with a command-line renderer is installed, it can be used as an optional
  external tool (procedural graphs in, pictures out, never bundled).
- Open-source libraries shipped with the program keep their licence next to them: Intel Open Image Denoise 2.5.1
  (Apache 2.0, `third_party/oidn`, DLLs copied next to the executable by `crates/shard-app/build.rs`).
- No third-party project data in the repository: test projects and their exports stay on the contributor's machine.

## License

[Apache License 2.0](LICENSE). Third-party notices in [`NOTICE`](NOTICE).

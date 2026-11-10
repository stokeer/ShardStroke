//! Headless ShardStroke engine: everything that turns a Painter project into pixels and can be measured, with
//! no window, no GPU and no UI. `shard-app` is a front end over this crate.
//!
//! * [`project`]: open a `.spp` (stacks, mesh, resources, bakes) as a plain value.
//! * [`compose`]: evaluate a texture set (mesh maps by name, stroke rasteriser, compositor options).
//! * [`persist`]: ShardStroke project files (`.shard`): edits on top of a base scene.
//! * [`measure`]: compare the result with Painter's own exports, channel by channel (docs/VALIDATION.md).
//! * [`mesh`], [`strokes`], [`bitmaps`], [`raster`]: supporting data and image operations.

pub mod bitmaps;
pub mod compose;
pub mod measure;
pub mod mesh;
pub mod oracle;
pub mod persist;
pub mod project;
pub mod raster;
pub mod strokes;
pub mod text;

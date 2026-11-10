# Intel Open Image Denoise

ShardStroke loads Open Image Denoise 2.5.1 at run time for the "AI denoise when still" option of the
path-traced viewport (`crates/shard-app/src/oidn.rs`). Without it the program works and the option shows
"Not installed".

The DLLs are not in the repository. To build with them, download `oidn-2.5.1.x64.windows.zip` from
https://github.com/RenderKit/oidn/releases/tag/v2.5.1 and copy the contents of its `bin` folder into
`third_party/oidn/bin/`. The build script copies them next to the executable.

License: Apache 2.0 (`LICENSE.txt`, and the `third-party-programs*.txt` files for its dependencies).

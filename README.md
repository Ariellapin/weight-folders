# Weight Folders

A Windows desktop tool that scans a drive or folder and shows where the space
goes, as a zoomable treemap next to a folder tree.

## Features

- **Scan** any drive or folder recursively. The scan is parallel (rayon) and
  shows live progress; it can be cancelled.
- **Treemap + tree panel.** Rectangles are proportional to size and nest as
  deep as the pixels allow. Files are colored on a blue → green → red scale
  (log-scaled) from the smallest to the largest file visible in the current
  view; a legend in the corner shows the range. Folders are gray. Double-click
  a folder to zoom in, use the breadcrumb, the **Up** button or **Backspace**
  to zoom out. Hover for the full path and item counts.
- **Open / Reveal / Copy path / Delete** from a right-click menu on any item in
  the treemap, the tree panel or the search results. Delete moves the item to
  the Windows Recycle Bin after a confirmation dialog. The **Delete** key works
  on the selected item, **Enter** opens a file or zooms into a folder.
- **Snapshots.** After a scan the whole tree is saved under
  `%LOCALAPPDATA%\weight-folders\data\snapshots\`. Scanning the same root again
  loads the snapshot instantly, then re-walks the disk in the background and
  reports what changed (added / removed / changed items and the net size delta).
  The start screen lists recent snapshots.
- **Search** the loaded snapshot by name (**Ctrl+F**). Case-insensitive
  substring match, or `*` / `?` wildcards such as `*.log`. Results can be
  filtered to files or folders and by minimum size; click a result to jump to
  it.

## Download

Grab the latest build from the
[Releases page](https://github.com/Ariellapin/weight-folders/releases/latest):

- `weight-folders-<version>-x64.msi` — installer. Puts the app in
  `Program Files\Weight Folders` and adds a Start Menu shortcut.
- `weight-folders-<version>-x64-portable.zip` / `weight-folders.exe` — no
  install needed, just run it.

The binaries are not code-signed, so Windows SmartScreen may show a
"Windows protected your PC" prompt the first time; choose *More info* →
*Run anyway*.

Releases are produced by the `Release` GitHub Actions workflow whenever a
`v*` tag is pushed:

```bash
git tag v0.1.0 && git push origin v0.1.0
```

The MSI is defined in `wix/main.wxs` (WiX v5, installed as a local dotnet
tool from `.config/dotnet-tools.json`).

## Building

```bash
cargo build --release
```

The binary is `target\release\weight-folders.exe`. Pass a folder on the command
line to start scanning it immediately:

```bash
target\release\weight-folders.exe "C:\Users\Me\Downloads"
```

`rust-toolchain.toml` pins `stable-x86_64-pc-windows-gnullvm`, which links with
[llvm-mingw](https://github.com/mstorsjo/llvm-mingw) (`winget install
MartinStorsjo.LLVM-MinGW.UCRT`). If you have Visual Studio Build Tools
installed you can delete that file and build with the default MSVC toolchain
instead.

## Tests

```bash
cargo test
```

Covers the scanner (sizes, counts, cancel), snapshot round-trip and diffing,
tree edits after deletes, the squarified layout and the wildcard matcher.

## Layout

| Path | Purpose |
|---|---|
| `src/model.rs` | Arena tree (`Node`, `Tree`), path lookup, subtree removal |
| `src/scanner.rs` | Parallel directory walk with progress and cancel |
| `src/snapshot.rs` | Snapshot files (MessagePack + zstd), recent list, diff |
| `src/app.rs` | State machine: start → scanning → ready, background jobs |
| `src/actions.rs` | Open, reveal in Explorer, move to Recycle Bin |
| `src/ui/treemap.rs` | Squarified treemap layout, painting, hit-testing |
| `src/ui/tree_panel.rs` | Collapsible folder tree |
| `src/ui/search.rs` | Search box, matcher, results panel |
| `src/ui/start.rs` | Start and scanning screens |
| `src/ui/dialogs.rs` | Delete confirmation and error toast |

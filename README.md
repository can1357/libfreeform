# libfreeform

Parser for Apple **Freeform** pasteboard data — Rust core, WebAssembly bindings for Node, Bun, Deno, and browsers.

[![CI](https://github.com/can1357/libfreeform/actions/workflows/ci.yml/badge.svg)](https://github.com/can1357/libfreeform/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/libfreeform)](https://crates.io/crates/libfreeform)
[![npm](https://img.shields.io/npm/v/libfreeform)](https://www.npmjs.com/package/libfreeform)

When you copy board items in Freeform, macOS/iPadOS writes the same selection to the pasteboard in several parallel flavors. None of the `com.apple.freeform.*` formats are documented; this library decodes them anyway — reverse-engineered, fixture-verified, no Apple frameworks required:

| Pasteboard flavor | What it carries | Decoder |
|---|---|---|
| `com.apple.drawing` | PencilKit `PKDrawing`: every freehand stroke with per-point position, width, pressure, plus ink family/color/opacity | `decodePkDrawing` |
| `com.apple.freeform.CRLNativeData` | The native object graph: board-item frames (position/size/rotation), fills, text, shape outlines, embedded ink | `decodeCrlNative` |
| `com.apple.freeform.TSUDescription` | Board-item class manifest (`CRLWPStickyNoteItem`, `CRLTableItem`, …) + routing hints | `parseTsuDescription` |

`decodePasteboard` assembles whatever flavors are present into one normalized selection. Decoding is **best-effort by design**: a damaged or unsupported tier degrades to a missing tier, and no input — truncated, corrupt, or hostile — panics or throws unexpectedly. The wire formats are documented in [`docs/FORMAT.md`](docs/FORMAT.md).

## Rust

```sh
cargo add libfreeform
```

```rust
use libfreeform::{FreeformBlobs, decode_pasteboard};

let blobs = FreeformBlobs {
    drawing: std::fs::read("pb_dump/com_apple_drawing").ok(),
    crl_native: std::fs::read("pb_dump/com_apple_freeform_CRLNativeData").ok(),
    tsu_description: std::fs::read("pb_dump/com_apple_freeform_TSUDescription").ok(),
    render_png: None,
};
let decoded = decode_pasteboard(blobs);

for stroke in decoded.drawing.iter().flat_map(|d| &d.strokes) {
    println!("{} stroke, {} points, {}", stroke.ink_type, stroke.points.len(), stroke.color);
}
for item in decoded.native.iter().flat_map(|n| &n.items) {
    println!("{}: frame={:?} text={:?}", item.class_name, item.frame, item.text);
}
```

The optional `serde` feature derives `Serialize`/`Deserialize` (camelCase field names) on all decoded types. Full API docs: [docs.rs/libfreeform](https://docs.rs/libfreeform).

## JavaScript / TypeScript

```sh
npm install libfreeform   # or: bun add libfreeform
```

The package is a single ESM module backed by WebAssembly. The wasm initializes at import — no init call, no async ceremony:

```js
import { decodePasteboard } from 'libfreeform';
import { readFileSync } from 'node:fs';

const decoded = decodePasteboard({
  drawing: readFileSync('pb_dump/com_apple_drawing'),
  crlNative: readFileSync('pb_dump/com_apple_freeform_CRLNativeData'),
  tsuDescription: readFileSync('pb_dump/com_apple_freeform_TSUDescription'),
});
console.log(decoded.drawing?.strokes.length, decoded.native?.items);
```

- **Node ≥ 20 / Bun** — works as-is (the `node` export condition loads the wasm synchronously from disk).
- **Deno** — `deno add npm:libfreeform`, then import as above; run with `-R` so the module may read its own `.wasm` file.
- **Browsers via a bundler** (Vite, webpack 5, Rollup) — works as-is; the wasm is referenced with the standard `new URL(..., import.meta.url)` asset pattern.
- **Browsers via CDN** — `import { decodePasteboard } from 'https://esm.sh/libfreeform'`.

Every function is fully typed (`index.d.ts` ships with the package); decoded objects use camelCase keys mirroring the Rust types.

### JS API

| Function | Purpose |
|---|---|
| `decodePasteboard(blobs)` | Assemble all present flavors into one `FreeformPasteboard`; failed tiers degrade to `undefined` |
| `decodePkDrawing(bytes)` | `com.apple.drawing` → strokes with per-point pressure/width (throws on unknown versions) |
| `decodeCrlNative(bytes, tsu?)` | `CRLNativeData` (+ optional `TSUDescription` join) → board items with frames/fills/text |
| `parseTsuDescription(bytes)` | `TSUDescription` → ordered `{ className, hints }` entries |
| `classifyBlob(name, bytes)` | Route a captured file to its flavor by filename + content signature |
| `isPkDrawing(bytes)` | Cheap `"wrd"` magic check |
| `hasFreeformContent(blobs)` | True when a flavor that can carry content is present |

## Capturing pasteboard data

`pbpaste` only sees text. To capture the Freeform flavors, copy something in Freeform, then dump every flavor to disk (macOS):

```sh
swift tools/dump_pasteboard.swift pb_dump   # -> pb_dump/<flavor_name>
```

Feed the resulting files to `decodePasteboard` / `classifyBlob` as shown above. [`docs/FORMAT.md`](docs/FORMAT.md) describes each flavor's layout and how new shape types were verified.

## Stability

The `com.apple.freeform.*` formats are private and can change with any Freeform/OS update (fixtures were captured on macOS 26.x). `PKDrawing` decoding is the most stable tier; the `CRLNativeData` heuristics are verified against real multi-shape boards but should be treated as best-effort. Gate imports on the decoded result, not on assumptions about the source version — that is how the API is shaped.

## Repository layout

- `src/` — the parser (pure Rust rlib; published to crates.io)
- `libfreeform-wasm/lib.rs` — target-gated wasm-bindgen module; the same crate emits it as a cdylib for npm
- `npm/` — the npm package: entry points, types, build script
- `tests/` — cross-runtime smoke tests (Node, Bun, Deno, headless Chromium)
- `docs/FORMAT.md` — the reverse-engineered format documentation
- `tools/dump_pasteboard.swift` — pasteboard capture utility

### Development

```sh
cargo test --all-features                  # Rust unit + fixture tests
cargo clippy --all-targets --all-features -- -D warnings
cargo +nightly fmt --all                   # rustfmt.toml uses nightly-only options
bunx @biomejs/biome check --write .        # JS/JSON lint + format
node npm/build.mjs                         # build the npm package into npm/dist
node tests/setup.mjs && node tests/smoke.mjs   # runtime smoke (also: bun/deno)
```

Building the npm package needs the `wasm32-unknown-unknown` target, a `wasm-bindgen` CLI matching the pin in root `Cargo.toml`, and optionally `wasm-opt` (binaryen). See [`RELEASING.md`](RELEASING.md) for the release process.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.

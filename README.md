# libfreeform

Parser for Apple **Freeform** pasteboard data — Rust core, WebAssembly bindings for Node, Bun, Deno, and browsers.

[![CI](https://github.com/can1357/libfreeform/actions/workflows/ci.yml/badge.svg)](https://github.com/can1357/libfreeform/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/libfreeform)](https://crates.io/crates/libfreeform)
[![npm](https://img.shields.io/npm/v/libfreeform)](https://www.npmjs.com/package/libfreeform)

Freeform writes one selection in several parallel pasteboard flavors. This library snapshots every flavor, decodes supported tiers independently, and retains unknown records and bytes for newer decoders:

| Pasteboard flavor | Decoded data |
|---|---|
| `com.apple.drawing` | PencilKit spline controls, full affine transforms, proven point channels, exact ink identifiers, and raw stroke records |
| `com.apple.freeform.CRLNativeData` | Typed, record-bounded board items: geometry/style, shapes, text, connectors, tables, media, groups, and native ink |
| `com.apple.freeform.TSUDescription` | Ordered item classes and recursive routing hints |
| `CRLNativeMetadata`, `CRLAsset.*`, state/style/text/render flavors | Correlation metadata, embedded assets, state flags, style/text selections, and PNG/TIFF/PDF fallbacks |

`decodePasteboard` returns a `decoded`, `failed`, or `absent` outcome for each independent tier. A damaged TSU manifest cannot discard valid native data; mismatched tiers are reported instead of partially joined. Private CRL formats remain OS-version fragile, so unsupported fields retain their bounded raw records rather than receiving guessed values. The wire formats are documented in [`docs/FORMAT.md`](docs/FORMAT.md).

## Rust

```sh
cargo add libfreeform
```

```rust
use libfreeform::{
    FreeformBlobs, FreeformFlavor, FreeformTier, decode_pasteboard,
};

let flavors = [
    ("com.apple.drawing", "pb_dump/flavor-000003.bin"),
    (
        "com.apple.freeform.CRLNativeData",
        "pb_dump/flavor-000001.bin",
    ),
    (
        "com.apple.freeform.TSUDescription",
        "pb_dump/flavor-000002.bin",
    ),
]
.into_iter()
.filter_map(|(uti, path)| {
    std::fs::read(path)
        .ok()
        .map(|bytes| FreeformFlavor { uti: uti.into(), bytes })
})
.collect();

let decoded = decode_pasteboard(FreeformBlobs {
    change_count: None,
    flavors,
});

if let FreeformTier::Decoded(drawing) = decoded.drawing {
    for stroke in drawing.strokes {
        println!("{}: {} spline controls", stroke.ink_type, stroke.points.len());
    }
}
if let FreeformTier::Decoded(native) = decoded.native {
    for item in native.items {
        println!("{:?}: {:?}", item.class_name, item.kind);
    }
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

const bytes = path => new Uint8Array(readFileSync(path));
const decoded = decodePasteboard({
  flavors: [
    { uti: 'com.apple.drawing', bytes: bytes('pb_dump/flavor-000003.bin') },
    {
      uti: 'com.apple.freeform.CRLNativeData',
      bytes: bytes('pb_dump/flavor-000001.bin'),
    },
    {
      uti: 'com.apple.freeform.TSUDescription',
      bytes: bytes('pb_dump/flavor-000002.bin'),
    },
  ],
});

if (decoded.drawing.status === 'decoded') {
  console.log(decoded.drawing.value.strokes.length);
}
if (decoded.native.status === 'decoded') {
  console.log(decoded.native.value.items);
}
```

- **Node ≥ 20 / Bun** — works as-is (the `node` export condition loads the wasm synchronously from disk).
- **Deno** — `deno add npm:libfreeform`, then import as above; run with `-R` so the module may read its own `.wasm` file.
- **Browsers via a bundler** (Vite, webpack 5, Rollup) — works as-is; the wasm is referenced with the standard `new URL(..., import.meta.url)` asset pattern.
- **Browsers via CDN** — `import { decodePasteboard } from 'https://esm.sh/libfreeform'`.

Every function is fully typed (`index.d.ts` ships with the package); decoded objects use camelCase keys mirroring the Rust types.

### JS API

| Function | Purpose |
|---|---|
| `decodePasteboard(snapshot)` | Decode exact ordered flavors into independent tier outcomes while retaining assets, renders, state, text, style, unknown flavors, and diagnostics |
| `decodePkDrawing(bytes)` | Decode local PencilKit spline controls, transforms, proven point channels, ink metadata, and raw records |
| `decodeCrlNative(bytes, tsu?)` | Decode record-bounded CRL items and optionally perform a strict TSU join |
| `parseTsuDescription(bytes)` | Decode ordered classes and recursive typed hint values |
| `classifyBlob(name, bytes)` | Classify exact UTIs/dump aliases and validated drawing/render signatures |
| `isPkDrawing(bytes)` | Check the `"wrd"` signature without crossing the Wasm boundary |
| `hasFreeformContent(snapshot)` | Test whether a snapshot contains an importable structured, asset, text/style, or rendered flavor |

## Capturing pasteboard data

`pbpaste` only sees text. Copy something in Freeform, then capture one atomic snapshot:

```sh
swift tools/dump_pasteboard.swift pb_dump
```

The destination contains `manifest.json` plus collision-safe `flavor-NNNNNN.bin` files. The manifest preserves exact UTIs, ordering, byte counts, and the stable pasteboard `changeCount`; capture retries if the pasteboard changes and atomically replaces an older snapshot without stale conditional assets.

## Stability

The `com.apple.freeform.*` formats are private and can change with any Freeform or OS update. CRL records are decoded only inside established object boundaries, and unknown fields remain available as raw bytes. `FreeformCompatibility` and per-tier failures let callers choose a render fallback without confusing absence, truncation, unsupported versions, or correlation failures.

## Repository layout

- `src/` — the parser (pure Rust rlib; published to crates.io)
- `libfreeform-wasm/lib.rs` — target-gated wasm-bindgen module; the same crate emits it as a cdylib for npm
- `npm/` — the npm package: entry points, types, build script
- `tests/` — cross-runtime smoke tests (Node, Bun, Deno, headless Chromium)
- `docs/FORMAT.md` — the reverse-engineered format documentation
- `tools/dump_pasteboard.swift` — pasteboard capture utility
- `tools/release.mjs` — synchronized Cargo/npm version commit and annotated-tag publisher

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

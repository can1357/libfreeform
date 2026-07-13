# Releasing

Releases are tag-driven: pushing `v<version>` runs `.github/workflows/release.yml`, which verifies the crate, then publishes `libfreeform` to **crates.io** and **npm** using OIDC **trusted publishing** — CI holds no long-lived registry tokens.

## Per-release flow

From a clean, current branch:

```sh
# Preview the exact version/commit/tag without mutating anything.
bun tools/release.mjs --dry-run 1.0.0

# Update Cargo + npm, refresh Cargo.lock, commit, tag, and atomically push.
bun tools/release.mjs 1.0.0
```

The script requires synchronized Cargo/npm versions, a new `v<version>` tag,
an `origin` remote, and no uncommitted changes. It creates
`chore: bumped to version <version>`, creates an annotated tag, then atomically
pushes the branch and tag. CI runs `verify` (tests + npm build + smoke), then
`publish-crate` and `publish-npm` in parallel. npm provenance attestations are
generated automatically because the publish goes through trusted publishing.

If a process dies after staging only those three version files, inspect the
diff and use `bun tools/release.mjs --resume <version>`. Resume refuses every
other staged, unstaged, or untracked file.

## One-time registry setup

Trusted publishing must be configured on each registry **after the package name first exists there**, so the very first release is published locally:

### First publish (bootstrap)

```sh
# crates.io — needs a token from https://crates.io/settings/tokens
cargo login
cargo publish --locked

# npm — needs `npm login`
node npm/build.mjs
cd npm && npm publish --access public
```

### crates.io trusted publisher

Docs: <https://crates.io/docs/trusted-publishing>

1. On crates.io: **libfreeform → Settings → Trusted Publishing → Add**.
2. Repository owner `can1357`, repository `libfreeform`, workflow filename `release.yml`. Leave the environment empty (the workflow doesn't use one).

The workflow's `publish-crate` job exchanges a GitHub OIDC token for a 30-minute crates.io token via [`rust-lang/crates-io-auth-action`](https://github.com/rust-lang/crates-io-auth-action); the job needs (and has) `id-token: write`.

### npm trusted publisher

Docs: <https://docs.npmjs.com/trusted-publishers>

1. On npmjs.com: **libfreeform → Settings → Trusted Publisher → GitHub Actions**.
2. Organization/user `can1357`, repository `libfreeform`, workflow filename `release.yml`. Leave the environment empty.

Requirements the workflow already satisfies: `id-token: write` permission, npm CLI ≥ 11.5.1 (Node 24 + `npm install -g npm@latest`), and a `repository.url` in `npm/package.json` matching the GitHub repo (required for the automatic provenance to verify).

Optional hardening for either registry: create a GitHub **environment** (e.g. `release`) with required reviewers, add `environment: release` to the publish jobs, and name that environment in both registry configs.

## Version pins that matter

- **wasm-bindgen**: the crate pin in root `Cargo.toml` (`wasm-bindgen = "=x.y.z"`), the `WASM_BINDGEN_VERSION` env in both workflows, and your locally installed `wasm-bindgen` CLI must all match. `npm/build.mjs` fails loudly when the CLI drifts. To upgrade: bump the crate pin, `cargo update -p wasm-bindgen`, bump both workflow envs, and `cargo install wasm-bindgen-cli --version <new> --locked`.
- **wasm-opt** (binaryen) is optional at build time — the build warns and ships unoptimized wasm without it — but CI installs it so published artifacts are always optimized.

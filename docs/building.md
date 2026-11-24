# Build from source

Monoptah supports macOS, Linux, and Windows. You need Node.js 22+ and a current stable Rust toolchain. Install and sign in to at least one agent CLI using [provider setup](providers.md).

## Platform and packages

The app is the Rust + GPUI native app, described under "Monocode Native preview" below. See the [native guide](native/README.md) for platform prerequisites, build output and checks, and [Linux installation](install.md) for runtime dependencies of the release packages.

The React/Tauri app (`src-tauri/`) was removed from this repository, so there is no `npm run tauri dev`, `build:linux`, `build:fedora` or `build:windows` any more. Packaging is done by the native release flow, see [release-flow](native/release-flow.md). Icons, entitlements and the updater public key used by that packaging live in `assets/native/`.

The experimental remote host has its own [build and packaging instructions](remote-access.md#release-packaging). Read [CONTRIBUTING.md](../CONTRIBUTING.md) for contribution guidelines and checks.

## Monocode Native preview

The GPUI app uses its own Cargo workspace (`crates/monocode-gpui` and `crates/monocode-core`).

Requires Node.js 22+, stable Rust, and Xcode with macOS components on macOS.
Native commands do not need npm dependencies installed.

```sh
npm run native:dev     # launch with a separate development database
npm run native:build   # create an optimized app using installed MonoCode data
npm run native:check   # run native checks (full)
npm run native:check:mac   # macOS-only remainder of what CI does not run
```

Type `r` then Enter in the development terminal to rebuild and restart. There is
no hot reload or automatic rebuild on save. Ctrl+C stops the runner and app.

The build command does not launch the app. On macOS, open the result with:

```sh
open "crates/monocode-gpui/target/release/Monocode Native.app"
```

The release app reads and writes your installed MonoCode data. It is locally
signed, without notarization or automatic updates. Linux/Windows GUI builds
remain unverified.

See the [native guide](native/README.md) for data paths, output locations,
fixtures and preview limitations.

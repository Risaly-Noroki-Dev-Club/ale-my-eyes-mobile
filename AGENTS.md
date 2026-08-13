# Repository Guidelines

## Project Structure & Module Organization

This is the independent mobile client for Ale, My Eyes!. The Rust workspace contains `ale-core/`, which owns configuration, cloud APIs, VAD, memory, action plans, and the encrypted remote protocol, and `ale-gui/`, the Slint mobile application. UI definitions live in `ale-gui/ui/`. Android-specific Rust code and resources are under `ale-gui/src/` and `ale-gui/android/res/`; iOS modules use `_ios.rs` names or `platform/ios.rs`. Packaging helpers are in `scripts/`, CI is in `.github/workflows/build.yml`, and technical references are in `docs/`. Do not edit generated `target/`, `ale-gui/android/build/`, or `ale-my-eyes-android/` files.

## Build, Test, and Development Commands

- `cargo fmt --all -- --check` verifies Rust formatting.
- `cargo check --workspace` checks host-compatible workspace code.
- `cargo test -p ale-core` runs the portable core test suite.
- `cargo check -p ale-gui --target aarch64-linux-android --lib` checks the Android client; install the Rust target and Android NDK first.
- `./scripts/package-android.sh` creates the arm64 APK in `ale-my-eyes-android/`. Set `ANDROID_HOME` or `ANDROID_SDK_ROOT`; `ANDROID_NDK_ROOT` is optional when the NDK is installed beneath the SDK.

The GUI crate intentionally compiles only for Android and iOS. A successful host check does not replace the Android target check.

## Coding Style & Naming Conventions

Use standard `rustfmt` output and four-space indentation. Follow Rust naming conventions: `snake_case` functions/modules, `CamelCase` types, and `SCREAMING_SNAKE_CASE` constants. Keep platform behavior behind narrow `#[cfg(target_os = "...")]` boundaries. Android remains a LAN command client; do not add desktop server or local automation responsibilities to it.

## Testing Guidelines

Place unit tests in a nearby `#[cfg(test)] mod tests` and name them after observable behavior, such as `pairing_uri_roundtrips`. Add regression tests for shared logic in `ale-core`; verify platform changes with the real Android target. No numeric coverage threshold is enforced, but new behavior should cover success and failure paths.

## Commit & Pull Request Guidelines

Recent commits use concise imperative subjects, for example `Refocus Android as LAN command client`; scoped prefixes such as `feat:` also appear. Keep each commit focused. Pull requests should explain behavior and platform impact, list commands run, link relevant issues, and include screenshots for Slint UI changes. Never commit API keys, keystores, generated APKs, or local configuration. Release CI requires `ANDROID_KEYSTORE_BASE64` and `ANDROID_KEYSTORE_PASSWORD` secrets.

# Android Release Checklist

Release is blocked until every required item below has evidence. Store local AVD output under ignored `test-artifacts/android-avd/` and attach the final screenshots and logs to the release record.

## Compatibility gate

- [ ] Desktop implements `REMOTE-PROTOCOL-V3.md` without fallback to older versions.
- [ ] Desktop validates chunk sequence, byte limit, frame count, and SHA-256 before ASR.
- [ ] Desktop cancellation and disconnect prevent later command execution.
- [ ] Coordinated desktop/mobile release versions are recorded.

## Automated checks

```bash
cargo fmt --all -- --check
cargo test -p ale-core
cargo check --workspace
cargo check -p ale-gui --target aarch64-linux-android --lib
./scripts/package-android.sh
```

- [ ] Formatting passes.
- [ ] All `ale-core` tests pass, including Noise, v1 rejection, audio limits, timeouts, cancellation, wrong IDs, disconnect, and confirmation.
- [ ] Host workspace check passes.
- [ ] Android arm64 target check passes with the release NDK.
- [ ] `zipalign -c`, `apksigner verify`, package, permission, version, and arm64-only checks pass.
- [ ] `ale-my-eyes-android/SHA256SUMS` matches the uploaded APK.

The app version name is `0.3.0`. `cargo-apk 0.10.0` derives `versionCode=16777984` from APK ID 1 and semver 0.3.0; it cannot represent literal versionCode 3 and rejects an override. Changing that value requires an intentional packaging-tool migration, not post-signature APK editing.

## Sequential AVD smoke

Run:

```bash
./scripts/smoke-android-avds.sh ale-my-eyes-android/ale-my-eyes-arm64.apk
```

- [ ] Pixel 7 Pro, API 34, arm64: install and launch.
- [ ] Pixel Tablet, API 35, arm64: install and launch.
- [ ] Permission revoke/grant state captured.
- [ ] Rotation and background/resume screenshots captured.
- [ ] Wi-Fi interruption performed.
- [ ] No package crash, native fatal signal, or ANR in saved logcat.

This script validates lifecycle behavior. It does not claim camera QR or microphone workflow completion.

## Manual v3 workflow on both AVDs

For each AVD, use a reachable deterministic v3 desktop and save screenshots/logs for:

- [ ] Scan and connect.
- [ ] First denial and permanent denial for camera and microphone; settings recovery.
- [ ] 1-second recording and preview.
- [ ] 10-second recording and preview.
- [ ] 60-second recording with automatic end.
- [ ] Reject preview and receive cancelled status.
- [ ] Confirm preview and receive completed status.
- [ ] Cancel while processing.
- [ ] Preview timeout recovery.
- [ ] Confirmation timeout recovery.
- [ ] Connection interruption clears recording and confirmation and requires rescan.
- [ ] Rotation and background/resume during idle, scanning, and processing.

## Release artifact

- [ ] Production keystore secrets were used; no temporary test certificate.
- [ ] APK filename, byte size, SHA-256, signer certificate digest, NDK, Rust toolchain, and build-tools versions are recorded.
- [ ] APK and `SHA256SUMS` are attached together.
- [ ] `docs/ANDROID-ISSUES.md` has no release-blocking item without evidence.

# Android Figma interface migration

Design source: Figma `KxBVpPWMx63te4qE45p205`, frames `2:2` (settings),
`2:144` (scan), and `2:267` (voice). The implementation keeps Rust/Slint and
Misaka-Network remote protocol v4. No desktop API changes are required.

## User flow

- Launch into Scan. Scan the desktop QR, wait for the encrypted handshake,
  then choose **开始语音控制**. A parsed QR alone is not a successful connection.
- In Voice, click the microphone to start and click again to stop/send.
  Cancel recording discards the request; the existing 60-second cap remains.
- Processing can continue across tabs. Confirmation is global and blocks the
  underlying controls. Navigation is disabled while recording or requesting
  microphone permission. Back cancels the active scan, recording or decision;
  otherwise it returns to Scan from another tab.
- Leaving Scan cancels scanning/connecting. Backgrounding cancels capture and
  recording, never implicitly sends audio. Returning to the app does not scan
  again. Connection loss requires explicit rescan.
- Settings persist in private Android SharedPreferences. Defaults: result
  speech on, haptics on, Chinese voice, automatic scan off. Automatic scan is
  attempted once on cold launch only. Language changes select TTS locale and
  do not translate text. Missing TTS data leaves text usable and reports an
  error. No audio, credentials or pairing secrets are saved in preferences.
- Device name/address and application/protocol versions are actual runtime
  values; the design's sample latency is intentionally omitted.

## Reproducible checks

Run the repository's format, core test, host check, Android target check and
APK packaging commands from AGENTS.md. Set the NDK compiler variables for
Android checks; packaging discovers the installed NDK automatically.

The independent preview tool compiles the **production Slint source**, renders
sample states with the software renderer, and checks real pointer hit targets.
It does not add a desktop mode to the mobile application:

```sh
cargo test --manifest-path tools/ui-preview/Cargo.toml --target-dir target/ui-preview-build
cargo run --manifest-path tools/ui-preview/Cargo.toml --target-dir target/ui-preview-build -- target/ui-preview
```

Output includes phone, 150%/200% text, landscape, and tablet screenshots for scan, connected,
voice, recording, processing, confirmation and settings. The sample desktop
address is preview-only. The host renderer can emit ICU segmentation-data
warnings; review the actual wrapping and images as well as its exit status.

Migration verification (2026-09-10): core suite 27/27, mobile preference and
operation invalidation tests 3/3, production UI pointer/Back checks, 35 rendered
states, host workspace check and Android arm64 check passed. Render review
fixed navigation expansion, focus interception, confirmation text wrapping
and compact-landscape recording controls. APK alignment, signature and
arm64 package verification passed with NDK 30.0.14904198.

Settings scroll follow-up: the Fluent ScrollView defaults to disabled drag
panning. All four mobile ScrollViews now explicitly enable
`mouse-drag-pan-enabled`, including scan, voice content and confirmation text.
The preview dispatches a press/move/release gesture over overflowing settings
at 200% text size and checks that content moves, preferences are unchanged,
and bottom navigation stays fixed. This regression check failed before the
fix (zero changed content pixels) and passes afterwards. The resulting image
is `<case>-settings-scrolled.png`. A follow-up uses the same native touch
entry point as Android, across large text, small screen, switch-row starts,
3x density and landscape. All five pass locally; the reported device-only
failure is still unconfirmed without the installed build and device details.

The `touch-camera-2` build also separates Y-only QR decoding from RGB preview,
respects YUV plane row/pixel strides, rotates previews using sensor/display
orientation, selects an advertised YUV output size, serializes camera reuse,
and surfaces camera disconnection/errors. Permission-dialog Pause no longer
cancels its own pending request; Stop still cancels when the app backgrounds.
Portable tests cover color, rotation, truncated input and stream selection.
Camera/permission runtime behaviour remains pending real-device validation.

The `rear-roi-3` build changes active scanning to a full-page, center-cover
camera preview above the fixed navigation, with a rectangular clear region
and dimmed surroundings. It selects only rear-facing cameras, preferring
Camera2 logical multi-camera groups at 1x; separate rear cameras are ranked
by their metadata's approximate equivalent focal length near normal wide
angle, not enumeration order. There is no silent front-camera fallback.
The on-screen camera label states rear/logical selection; the actual physical
sensor used inside a logical camera remains Android's decision. Pixel 7 Pro
hardware has not been attached or queried during this implementation.

The decoder receives only the rotated luminance crop corresponding to the
visible centered rectangle, accounting for cover scaling and screen geometry.
Layout changes update the shared region; mismatched rotation snapshots skip
decoding. Tests exercise crop boundaries, all four rotations, camera ranking,
and actual QR decoding for inside/outside/crossing-border placements. Nine
portable tests, five native-touch scenarios and 40 rendered UI states pass.
Camera preview PNGs use a synthetic color chart, not an actual camera feed.

## Validation boundaries and remaining release gates

- Android arm64 compilation and locally signed APK checks are separate from
  Android runtime tests. The local package uses the existing test signing key.
- No Android device was attached and no AVD was configured during migration.
  Camera QR, microphone, TTS locale availability, haptics, permission recovery,
  lifecycle, real desktop v4 interoperability and system Back need device tests.
- Controls expose Slint accessibility labels, roles, states, keyboard actions
  and modal focus. The current `i-slint-backend-android-activity 1.16.1` source
  has no Android accessibility-tree bridge. **TalkBack usability is not
  established, and remains a release blocker.** App TTS does not replace that
  bridge. Do not describe this build as accessible or release-ready without
  an actual TalkBack workflow test and any required backend remediation.
- System font scale is read at startup/resume. Verify 100%, 150%, 200%, narrow
  landscape, tablet, gesture navigation and cutout insets on Android devices.
- Existing `ANDROID-RELEASE-CHECKLIST.md` requirements remain applicable; no
  external upload or production release is performed by this migration.

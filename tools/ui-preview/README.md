# Production UI preview

This standalone development tool renders the mobile application's actual
Slint tree using the software renderer, without launching a GUI, microphone,
camera, network client or Android application.

From the repository root:

```sh
cargo test --manifest-path tools/ui-preview/Cargo.toml --target-dir target/ui-preview-build
cargo run --manifest-path tools/ui-preview/Cargo.toml --target-dir target/ui-preview-build -- target/ui-preview
```

Tests reuse the production preference decoder and operation invalidation code.
Running the tool also asserts navigation/microphone hit targets, recording
navigation locking and modal interception before rendering sample states.
Use the generated PNGs for layout review, not as Android runtime evidence.

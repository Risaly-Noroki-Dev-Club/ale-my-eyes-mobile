# Ale, My Eyes! Mobile

Mobile application for Ale, My Eyes!, providing voice interaction and visual assistance on Android and iOS.

This repository is intentionally independent from the desktop application. It owns its mobile runtime, Android packaging resources, and a private copy of `ale-core`; it has no source or Cargo dependency on `ale-my-eyes-desktop`.

## Android build

```bash
./scripts/package-android.sh
```

See the desktop client and CLI at https://github.com/Risaly-Noroki-Dev-Club/ale-my-eyes-desktop.

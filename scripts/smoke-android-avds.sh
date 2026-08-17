#!/usr/bin/env bash

set -euo pipefail

PACKAGE_NAME="com.alemyeyes"
AVDS=("Pixel_7_Pro" "Pixel_Tablet")
PORTS=("5554" "5556")

fail() {
    printf "AVD smoke test failed: %s\n" "$1" >&2
    exit 1
}

resolve_sdk_root() {
    local candidate=""
    for candidate in "${ANDROID_HOME:-}" "${ANDROID_SDK_ROOT:-}" "$HOME/Library/Android/sdk" "/usr/local/lib/android/sdk"; do
        if [ -n "$candidate" ] && [ -d "$candidate" ]; then
            printf "%s\n" "$candidate"
            return
        fi
    done
    fail "Android SDK not found"
}

wait_for_boot() {
    local adb="$1"
    local serial="$2"
    local attempts=0
    "$adb" -s "$serial" wait-for-device
    until [ "$("$adb" -s "$serial" shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" = "1" ]; do
        attempts=$((attempts + 1))
        [ "$attempts" -lt 180 ] || fail "$serial did not boot within 180 seconds"
        sleep 1
    done
}

run_avd() (
    local emulator="$1"
    local adb="$2"
    local avd="$3"
    local port="$4"
    local apk="$5"
    local output="$6"
    local serial="emulator-$port"
    local emulator_pid=""

    mkdir -p "$output"
    "$emulator" -avd "$avd" -port "$port" -no-snapshot-save -no-boot-anim \
        >"$output/emulator.log" 2>&1 &
    emulator_pid=$!

    cleanup_avd() {
        "$adb" -s "$serial" emu kill >/dev/null 2>&1 || true
        wait "$emulator_pid" 2>/dev/null || true
    }
    trap cleanup_avd EXIT

    wait_for_boot "$adb" "$serial"
    "$adb" -s "$serial" logcat -c
    "$adb" -s "$serial" uninstall "$PACKAGE_NAME" >/dev/null 2>&1 || true
    "$adb" -s "$serial" install "$apk" >"$output/install.txt"
    "$adb" -s "$serial" shell pm revoke "$PACKAGE_NAME" android.permission.CAMERA >/dev/null 2>&1 || true
    "$adb" -s "$serial" shell pm revoke "$PACKAGE_NAME" android.permission.RECORD_AUDIO >/dev/null 2>&1 || true
    "$adb" -s "$serial" shell monkey -p "$PACKAGE_NAME" -c android.intent.category.LAUNCHER 1 \
        >"$output/launch.txt"
    sleep 3
    "$adb" -s "$serial" exec-out screencap -p >"$output/portrait.png"

    "$adb" -s "$serial" shell settings put system accelerometer_rotation 0
    "$adb" -s "$serial" shell settings put system user_rotation 1
    sleep 2
    "$adb" -s "$serial" exec-out screencap -p >"$output/rotated.png"
    "$adb" -s "$serial" shell input keyevent HOME
    sleep 1
    "$adb" -s "$serial" shell monkey -p "$PACKAGE_NAME" -c android.intent.category.LAUNCHER 1 \
        >"$output/resume.txt"
    "$adb" -s "$serial" shell pm grant "$PACKAGE_NAME" android.permission.CAMERA
    "$adb" -s "$serial" shell pm grant "$PACKAGE_NAME" android.permission.RECORD_AUDIO
    "$adb" -s "$serial" shell dumpsys package "$PACKAGE_NAME" >"$output/package.txt"
    "$adb" -s "$serial" shell svc wifi disable
    sleep 1
    "$adb" -s "$serial" shell svc wifi enable
    sleep 2
    "$adb" -s "$serial" logcat -d >"$output/logcat.txt"

    if rg -i "FATAL EXCEPTION|Fatal signal|ANR in $PACKAGE_NAME" "$output/logcat.txt"; then
        fail "$avd reported a crash or ANR"
    fi
    printf "%s smoke test passed\n" "$avd" | tee "$output/result.txt"
)

main() {
    local apk="${1:-ale-my-eyes-android/ale-my-eyes-arm64.apk}"
    [ -f "$apk" ] || fail "APK not found: $apk"
    command -v rg >/dev/null 2>&1 || fail "rg is required"
    local sdk_root=""
    sdk_root=$(resolve_sdk_root)
    local emulator="$sdk_root/emulator/emulator"
    local adb="$sdk_root/platform-tools/adb"
    [ -x "$emulator" ] || fail "emulator not found"
    [ -x "$adb" ] || fail "adb not found"

    local available=""
    available=$("$emulator" -list-avds)
    local avd=""
    for avd in "${AVDS[@]}"; do
        printf "%s\n" "$available" | grep -Fx "$avd" >/dev/null || fail "missing AVD: $avd"
    done

    local timestamp=""
    timestamp=$(date -u +%Y%m%dT%H%M%SZ)
    local root="test-artifacts/android-avd/$timestamp"
    local index=0
    for avd in "${AVDS[@]}"; do
        run_avd "$emulator" "$adb" "$avd" "${PORTS[$index]}" "$apk" "$root/$avd"
        index=$((index + 1))
    done
    printf "AVD smoke artifacts: %s\n" "$root"
}

main "$@"

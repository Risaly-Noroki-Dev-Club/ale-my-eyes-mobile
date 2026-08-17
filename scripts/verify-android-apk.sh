#!/usr/bin/env bash

set -euo pipefail

EXPECTED_PACKAGE="com.alemyeyes"
EXPECTED_VERSION_NAME="0.3.0"
# cargo-apk 0.10 encodes apk_id=1 plus semver 0.3.0 into this value.
EXPECTED_VERSION_CODE="16777984"
EXPECTED_ABI="arm64-v8a"

fail() {
    printf "APK verification failed: %s\n" "$1" >&2
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

resolve_build_tools() {
    local sdk_root="$1"
    local newest=""
    newest=$(find "$sdk_root/build-tools" -mindepth 1 -maxdepth 1 -type d 2>/dev/null | sort -V | tail -n 1 || true)
    [ -n "$newest" ] || fail "Android build-tools not found"
    printf "%s\n" "$newest"
}

sha256_file() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{print $1}'
    else
        fail "sha256sum or shasum is required"
    fi
}

main() {
    [ "$#" -eq 1 ] || fail "usage: $0 path/to/app.apk"
    local apk="$1"
    [ -f "$apk" ] || fail "APK not found: $apk"
    command -v unzip >/dev/null 2>&1 || fail "unzip is required"

    local sdk_root=""
    local build_tools=""
    local badging=""
    local permissions=""
    local abis=""
    sdk_root=$(resolve_sdk_root)
    build_tools=$(resolve_build_tools "$sdk_root")

    "$build_tools/zipalign" -c -P 16 4 "$apk"
    "$build_tools/apksigner" verify --verbose --print-certs "$apk"

    badging=$("$build_tools/aapt" dump badging "$apk")
    printf "%s\n" "$badging" | grep -F "package: name='$EXPECTED_PACKAGE'" >/dev/null \
        || fail "unexpected package name"
    printf "%s\n" "$badging" | grep -F "versionName='$EXPECTED_VERSION_NAME'" >/dev/null \
        || fail "unexpected versionName"
    printf "%s\n" "$badging" | grep -F "versionCode='$EXPECTED_VERSION_CODE'" >/dev/null \
        || fail "unexpected cargo-apk versionCode"

    permissions=$("$build_tools/aapt" dump permissions "$apk")
    for permission in \
        android.permission.INTERNET \
        android.permission.RECORD_AUDIO \
        android.permission.MODIFY_AUDIO_SETTINGS \
        android.permission.CAMERA; do
        printf "%s\n" "$permissions" | grep -F "uses-permission: name='$permission'" >/dev/null \
            || fail "missing permission: $permission"
    done

    abis=$(unzip -Z1 "$apk" | sed -n 's#^lib/\([^/]*\)/.*#\1#p' | sort -u)
    [ "$abis" = "$EXPECTED_ABI" ] || fail "expected only $EXPECTED_ABI, found: ${abis:-none}"

    local digest=""
    local manifest=""
    digest=$(sha256_file "$apk")
    manifest="$(dirname "$apk")/SHA256SUMS"
    printf "%s  %s\n" "$digest" "$(basename "$apk")" > "$manifest"
    printf "Verified %s\nSHA-256: %s\n" "$apk" "$digest"
}

main "$@"

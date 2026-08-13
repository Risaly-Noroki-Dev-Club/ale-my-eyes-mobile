#!/usr/bin/env bash
# build-android-java.sh — 编译 Android Java 源码为 class 文件
# 用法: ./scripts/build-android-java.sh [android_jar_path]
#
# 需要: JDK 11+, ANDROID_HOME 或 ANDROID_SDK_ROOT 环境变量

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
JAVA_SRC="$PROJECT_ROOT/ale-gui/android/java"
BUILD_DIR="$PROJECT_ROOT/ale-gui/android/build"
CLASSES_DIR="$BUILD_DIR/classes"

# 查找 android.jar
find_android_jar() {
    local sdk_root="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
    if [ -z "$sdk_root" ]; then
        echo "错误: 未设置 ANDROID_HOME 或 ANDROID_SDK_ROOT" >&2
        exit 1
    fi

    # 使用 API 34 (target_sdk_version)
    local jar="$sdk_root/platforms/android-34/android.jar"
    if [ -f "$jar" ]; then
        echo "$jar"
        return
    fi

    # 回退到任意可用版本
    for jar in "$sdk_root"/platforms/android-*/android.jar; do
        if [ -f "$jar" ]; then
            echo "$jar"
            return
        fi
    done

    echo "错误: 未找到 android.jar，请安装 Android SDK platform" >&2
    exit 1
}

if [ ! -d "$JAVA_SRC" ]; then
    echo "未配置自定义 Android Java 源码，跳过"
    exit 0
fi

ANDROID_JAR="${1:-$(find_android_jar)}"
echo "使用 android.jar: $ANDROID_JAR"

# 清理旧的编译产物
rm -rf "$CLASSES_DIR"
rm -f "$BUILD_DIR/classes.dex"
mkdir -p "$CLASSES_DIR"

# 查找所有 Java 源文件
JAVA_FILES=$(find "$JAVA_SRC" -name "*.java" -type f)
if [ -z "$JAVA_FILES" ]; then
    echo "未找到 Java 源文件"
    exit 0
fi

echo "编译 Java 源文件..."
echo "$JAVA_FILES" | while read -r f; do
    echo "  $f"
done

# 编译 Java
javac \
    -source 11 \
    -target 11 \
    -classpath "$ANDROID_JAR" \
    -d "$CLASSES_DIR" \
    $JAVA_FILES

echo "编译完成: $CLASSES_DIR"

# 生成 DEX（可选，需要 d8 工具）
if command -v d8 &>/dev/null; then
    echo "生成 DEX 文件..."
    CLASS_FILES=$(find "$CLASSES_DIR" -name "*.class" -type f)
    d8 \
        --lib "$ANDROID_JAR" \
        --output "$BUILD_DIR" \
        $CLASS_FILES
    echo "DEX 文件已生成: $BUILD_DIR/classes.dex"
else
    echo "提示: 未找到 d8 工具，跳过 DEX 生成"
    echo "  安装 Android build-tools 后可自动生成 DEX"
fi

echo "构建完成"

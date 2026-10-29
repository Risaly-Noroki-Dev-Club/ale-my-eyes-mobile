# Ale, My Eyes! Android

> **Beta** — AME的第一个公开版本自此开始。

## 什么是AME

Ale, My Eyes! 是你口袋里的另一位伙伴。我们期待可以帮到你。

这个 Android 应用本身不运行人工智能模型。相反，它通过手机收集你的声音，安全地传给你的电脑，再把电脑的回应读给你听。

## 它是怎么工作的

你说话，ame借助whisper理解你在说什么，然后AME透过自带的本地模型理解屏幕上发生了什么，丢给那一群数据中心里面的神秘概率机器，把所有应该实现的东西丢到本地之后一个专门的控屏模型会帮你做完，然后啪！你知道大功告成了（当然实际上并不会炸）

## 此刻它能做什么（Beta 版）

- **语音对话**：说点什么吧
- **屏幕理解**：让电脑理解屏幕并描述上面的内容。
- **安全确认**：“完全访问”是个谎言！
- **隐私优先**：所有通信都在本地网络内完成，端到端加密；配对码只在内存中短暂使用，手机端从不保存（配对码由桌面端在启动、刷新地址或会话轮换时重新生成，有效期 120 秒）。（我也不知道你说了什么！）

## 兼容性

- Android 8.0（API 26）及以上
- 仅支持 arm64 设备（抱歉了所有的模拟器用户）
- 需要与桌面端处于同一局域网
- 仅支持 Misaka-Network 远程协议 v4（旧版本桌面端将被拒绝连接）

## 开发

```bash
# 检查代码格式
cargo fmt --all -- --check

# 运行核心测试
cargo test -p ale-core

# 检查主机端代码
cargo check --workspace

# 检查 Android 客户端（需要 Android NDK）
cargo check -p ale-gui --target aarch64-linux-android --lib
```

构建并验证签名 APK：

```bash
./scripts/package-android.sh
```

AVD 烟雾测试：

```bash
./scripts/smoke-android-avds.sh ale-my-eyes-android/ale-my-eyes-arm64.apk
```

## 文档

- [Android 架构](docs/ARCHITECTURE.md)
- [Misaka-Network 远程协议 v4](docs/REMOTE-PROTOCOL-V4.md)
- [发布检查清单](docs/ANDROID-RELEASE-CHECKLIST.md)
- [已知问题](docs/ANDROID-ISSUES.md)

## 致未来的使用者

我们知道，依赖技术来感知世界是一件需要信任的事。我们不敢说自己已经完美，但我们可以承诺：每一个设计决策背后，都站着一个真正需要它的人。

如果你正在使用这个应用，谢谢你愿意给我们一个机会。

感谢所有人的耐心等待和支持。

---

*Ale, My Eyes! 是一个独立的开源项目，与任何商业助盲服务无关。当然和AMA-10也是如此。*

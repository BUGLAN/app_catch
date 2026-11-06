# macOS 隐私权限与代码签名

本文说明：经 app_catch 启动的程序在 macOS 上如何拿到麦克风、摄像头等隐私权限，
以及为什么 app_catch 要用固定的自签名证书签名。

## 权限记在谁名下

macOS 的隐私权限（TCC）按**责任进程（responsible process）**判定，不看发起请求的进程本身：

```
终端里跑：  Terminal.app → zsh → 目标进程      → 责任进程 = Terminal
app_catch： app_catch.app → 目标进程           → 责任进程 = app_catch
```

所以从 app_catch 启动的命令、脚本、开发态程序请求麦克风时，系统弹出的是
「app_catch 想访问麦克风」，授权也记在 app_catch 名下。这与从终端启动时记在 Terminal 名下是同一个道理。

例外是把应用设为「像 Dock 一样打开」（`launch_mode = "open"`，见 [CONFIG.md](CONFIG.md#启动-app-应用macos)）：
应用交给系统打开、由 launchd 拉起，它自己就是责任进程，权限与从 Dock 打开完全一致，已有的授权直接可用。
把已安装的应用当成快捷启动项时用这种方式。

结论：

- **授权一次，全部共享**：给 app_catch 开了麦克风后，所有经它启动的程序都能直接使用，不会逐个询问；
- **用开发态启动 app_catch 时不同**：`pnpm tauri dev` 从终端起 app_catch，整条链的责任进程是终端，
  此时看到的弹窗和授权都属于终端，与安装版的行为不一致属正常现象。

## 用途声明（Info.plist）

责任进程的 `Info.plist` 里缺少某项权限的用途说明（`NS*UsageDescription`）时，
子进程一请求该权限就会被系统**直接杀掉**，表现为应用崩溃或权限被静默拒绝。

因此 [`src-tauri/Info.plist`](../src-tauri/Info.plist) 预先声明了常用几项（Tauri 打包时会合并进最终的 Info.plist）：

| 权限 | 键 |
| --- | --- |
| 麦克风 | `NSMicrophoneUsageDescription` |
| 摄像头 | `NSCameraUsageDescription` |
| 语音识别 | `NSSpeechRecognitionUsageDescription` |
| 蓝牙 | `NSBluetoothAlwaysUsageDescription` |
| 定位 | `NSLocationUsageDescription` / `NSLocationWhenInUseUsageDescription` |
| 通讯录 | `NSContactsUsageDescription` |
| 日历 | `NSCalendarsUsageDescription` / `NSCalendarsFullAccessUsageDescription` |
| 提醒事项 | `NSRemindersUsageDescription` / `NSRemindersFullAccessUsageDescription` |
| 照片 | `NSPhotoLibraryUsageDescription` |
| 自动化（Apple Events） | `NSAppleEventsUsageDescription` |
| 本地网络 | `NSLocalNetworkUsageDescription` |

被启动的程序需要的权限不在表里时，往这个文件里补一项即可。

## 需要手动开启的权限

下面这几类权限同样按责任进程判定，但**不会弹窗**，需要在「系统设置 → 隐私与安全性」里
手动把 **app_catch** 加进对应列表：

- 屏幕录制（截屏、录屏）
- 辅助功能（模拟键鼠、读取其他应用的界面）
- 输入监控（全局快捷键、键盘监听）
- 完全磁盘访问

这些权限范围很大，开启后所有经 app_catch 启动的程序都会获得，请按需开启。

## 为什么要固定证书签名

TCC 记录授权时按「bundle id + 签名要求（designated requirement）」识别应用：

| 签名方式 | 签名要求 | 升级后 |
| --- | --- | --- |
| ad-hoc（未配置证书时的默认） | `cdhash H"..."`，每次构建都变 | 被当成另一个应用，授权失效；常见表现是**既不弹窗、也不放行** |
| 固定自签名证书 | `identifier "com.appcatch.launcher" and certificate leaf = H"<证书指纹>"` | 只要证书和 bundle id 不变，授权一直有效 |

因此：

- `tauri.conf.json` 的 `identifier` 以后不能再改；
- 签名证书必须长期复用并妥善备份。换证书等于所有用户的授权失效一次；
- 从 ad-hoc 切换到固定证书的那一次升级，授权会重置一次，属一次性成本。

自签名证书只解决授权保留问题，**不能通过 Gatekeeper**：首次下载安装后仍会提示「已损坏，无法打开」，
需要执行一次：

```bash
xattr -cr /Applications/app_catch.app
```

### hardened runtime

`tauri.conf.json` 里设置了 `bundle.macOS.hardenedRuntime: false`。hardened runtime 只在公证（notarization）时必需，
自签名证书无法公证，开启它没有收益，反而要求额外的 entitlements：
例如缺少 `com.apple.security.device.audio-input` 时，即使用户授权了麦克风，app_catch 启动的程序也拿不到。
以后改用 Developer ID 并公证时，需要同时开启 hardened runtime 并补上这些 entitlements。

## 证书的生成与配置（一次性）

1. 在本机生成证书（默认输出到 `~/.app_catch-signing`，脚本拒绝覆盖已有证书）：

   ```bash
   scripts/gen-macos-signing-cert.sh
   ```

2. 配置 GitHub 仓库 secret（脚本结尾会打印同样的命令）：

   ```bash
   base64 -i ~/.app_catch-signing/cert.p12 | gh secret set MACOS_SIGNING_CERT_P12
   ```

   ```bash
   gh secret set MACOS_SIGNING_CERT_PASSWORD < ~/.app_catch-signing/password.txt
   ```

3. 把 `~/.app_catch-signing` 备份到安全的地方。

secret 名字刻意不用 Tauri 的 `APPLE_CERTIFICATE`：Tauri 读到它会走自己的导入流程，
那条流程只认 Apple 签发、带 Team ID 的证书，自签名证书会直接报错。
`release.yml` 自己把证书导入临时 keychain，只把 `APPLE_SIGNING_IDENTITY` 交给 Tauri，
打包后再校验签名要求里确实带有证书指纹；未配置 secret 时退回 ad-hoc 签名并在 Actions 里告警。

## 排查

查看安装版的签名要求，确认是否用固定证书签名：

```bash
codesign -d -r- /Applications/app_catch.app
```

授权状态异常（不弹窗、直接拒绝）时，清掉 app_catch 的某项授权，下次使用时会重新弹窗：

```bash
tccutil reset Microphone com.appcatch.launcher
```

把 `Microphone` 换成 `Camera`、`SpeechRecognition` 等即可清理其他权限；省略服务名的写法 `tccutil reset All com.appcatch.launcher` 会清掉全部。

# 安装与使用 bili

## Homebrew（macOS / Linux）

```sh
brew install chick26/tap/bili
bili --help
```

Homebrew 会同时安装 FFmpeg，之后使用 `brew upgrade bili` 升级。

## 下载便携包

在 [GitHub Releases](https://github.com/chick26/bilibili-downloader-cli/releases/latest) 选择对应平台：

| 平台 | 文件名包含 |
| --- | --- |
| macOS，Apple 芯片，macOS 12+ | `macos-arm64.tar.gz` |
| macOS，Intel 芯片，macOS 12+ | `macos-x86_64.tar.gz` |
| Linux，Intel / AMD 64 位 | `linux-x86_64.tar.gz` |
| Linux，ARM 64 位 | `linux-arm64.tar.gz` |
| Windows 10/11，Intel / AMD 64 位 | `windows-x86_64.zip` |

每个包都包含 `bili`、独立的 `ffmpeg`、使用说明和许可证。无需安装 Rust、Node.js 或另行安装 FFmpeg；请将这两个可执行文件放在同一目录。Linux 包使用 musl 和静态 TLS，系统仍需具备 CA 证书以访问 HTTPS。

macOS / Linux 解压后在终端进入该目录：

```sh
./bili --help
./bili up videos 504018708
./bili download BV17x411w7KC --output ./downloads --no-extras
```

Windows 解压后在该目录打开 PowerShell：

```powershell
.\bili.exe --help
.\bili.exe up videos 504018708
.\bili.exe download BV17x411w7KC --output .\downloads --no-extras
```

也可将目录加入 PATH，之后直接使用 `bili`。这是命令行程序，请从终端启动。

macOS 便携包没有 Apple 开发者公证。如果系统阻止运行，可在确认下载来源后对解压目录中的两个文件执行：

```sh
xattr -d com.apple.quarantine ./bili ./ffmpeg
```

如果文件没有该属性，无需处理；也可以通过 Homebrew 安装。

Release 提供 `SHA256SUMS`，可以在下载后使用 `shasum -a 256 文件名`（macOS）、`sha256sum 文件名`（Linux）或 `Get-FileHash 文件名 -Algorithm SHA256`（Windows）核对。

## 登录、下载和恢复

```sh
bili auth login
bili auth status
bili download BV17x411w7KC --output ./downloads --no-extras
bili tasks list
bili resume TASK_ID
```

便携包中运行上述命令时，macOS / Linux 使用 `./bili`，Windows 使用 `.\bili.exe`。B 站扫码登录需要本人确认，最高画质和受限内容取决于账号权限。Ctrl-C 保存已完成分片，使用打印的任务 ID 恢复。

Agent 和脚本可增加 `--json` 获取单个 JSON 结果。完整参数见 [项目 README](https://github.com/chick26/bilibili-downloader-cli#readme)。

## 许可与对应源码

bili 源码遵循随包提供的 MIT 许可并保留上游作者声明。本软件使用 [FFmpeg](https://ffmpeg.org/) 独立命令行程序，其许可为 LGPL-2.1-or-later；未启用 GPL、nonfree 或外部编解码库。同一 Release 提供与内置二进制对应的 `ffmpeg-8.1.3.tar.xz` 完整源码，构建方式与各组件许可位于包内 `licenses/`。

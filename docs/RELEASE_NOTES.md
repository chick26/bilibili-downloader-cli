首个可直接使用的 CLI 版本，支持 UID 投稿与收藏夹浏览、前台下载、JSON 输出和断点续传。音视频分片具备自动重试、超时和范围完整性校验。

安装方式与平台选择见 [安装说明](https://github.com/chick26/bilibili-downloader-cli/blob/main/docs/INSTALL.md)。

便携包内置 `bili` 与 `ffmpeg`，不需要 Rust、Node.js 或额外安装 FFmpeg。macOS 分 Apple 芯片与 Intel；Linux 分 x86_64 与 ARM64；Windows 提供 x64 ZIP。macOS 便携包未经 Apple 开发者公证，首次运行方式见安装说明。

macOS / Linux 也可使用 Homebrew：

```sh
brew install chick26/tap/bili
```

下载后可使用 `SHA256SUMS` 核对文件。B 站登录和账号权限仍由本人处理，扫码会话不随安装包分发。

本软件随包使用 [FFmpeg](https://ffmpeg.org/) 独立程序，采用 [LGPL-2.1-or-later](https://www.gnu.org/licenses/old-licenses/lgpl-2.1.html)。与内置二进制对应的完整源码位于本 Release 的 `ffmpeg-8.1.3.tar.xz`，构建脚本与许可位于包内 `licenses/`，未启用 GPL、nonfree 或外部编解码库。bili 自身保留上游 MIT 许可，Rust 依赖的许可文本也随包提供。

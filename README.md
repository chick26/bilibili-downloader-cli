# Bilibili Downloader CLI

B 站命令行下载器，基于 [lanyeeee/bilibili-video-downloader](https://github.com/lanyeeee/bilibili-video-downloader) 开发。支持视频下载、关键词搜索、UP 投稿与收藏夹浏览，以及供 Agent 使用的 JSON 输出。

下载支持断流自动重试、超时处理、分片完整性校验和断点续传。

## 安装

macOS / Linux：

```sh
brew install chick26/tap/bili
```

也可从 [Releases](https://github.com/chick26/bilibili-downloader-cli/releases/latest) 下载 macOS、Linux 或 Windows 便携包，解压后在终端使用。包内包含 FFmpeg，无需安装 Rust 或 Python。详见 [安装说明](docs/INSTALL.md)。

## 使用

```sh
# 扫码登录
bili auth login

# 下载视频，也支持完整视频 URL
bili download BV17x411w7KC -o ./downloads --no-extras

# 搜索 UP 或视频
bili search '影视飓风' --type user
bili search 'Rust 入门' --type video

# 按 UID 浏览 UP 投稿
bili up videos 504018708 --page 1

# 查看任务，恢复中断的下载
bili tasks list
bili resume TASK_ID
```

`--no-extras` 仅保留音视频，跳过弹幕、字幕、封面等附加内容。下载时按 Ctrl-C 会保存进度，可用任务 ID 继续。

## Agent / 脚本

命令加 `--json` 即可获得单个 JSON 结果，进度和诊断写入 stderr：

```sh
bili search 'Rust 入门' --type video --json
bili download BV17x411w7KC -o ./downloads --no-extras --json
```

## 更多说明

- 完整参数、收藏夹、画质选择、数据目录、退出码与源码构建：[使用指南](docs/USAGE.md)。
- 查看命令帮助：`bili --help` 或 `bili download --help`。
- 可用内容与画质取决于账号权限；遇到 HTTP 412 风控时，请稍后重试或按网页提示完成验证。登录不保证解除风控。
- 登录信息保存在本地数据目录，请勿提交到 Git 或公开分享。

## 许可与来源

本项目采用 [MIT License](LICENSE)，保留上游作者版权。来源与致谢见 [UPSTREAM.md](UPSTREAM.md)。便携包内的 FFmpeg 采用 LGPL-2.1-or-later，同一 Release 提供对应完整源码。

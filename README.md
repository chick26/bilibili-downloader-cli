# Bilibili Downloader CLI

基于 [lanyeeee/bilibili-video-downloader](https://github.com/lanyeeee/bilibili-video-downloader) 迭代的命令行下载器，面向终端和 Codex 等 Agent。共享 Rust 内核负责 UP 投稿与收藏夹浏览、视频解析、分片下载、断点续传与 FFmpeg 处理，`bili` 提供前台命令和 JSON 输出。

支持终端表格与 `--json` 机器可读输出。项目来源、上游基线和许可证说明见 [UPSTREAM.md](UPSTREAM.md)。

## 安装

macOS / Linux 推荐使用 Homebrew：

```sh
brew install chick26/tap/bili
bili --help
```

也可在 [Releases](https://github.com/chick26/bilibili-downloader-cli/releases/latest) 下载 macOS、Linux 或 Windows 的便携包。包内包含 `bili` 和 `ffmpeg`，解压后即可从终端使用，无需编译或安装 Rust。平台选择、首次运行和升级方法见 [安装说明](docs/INSTALL.md)。

便携包随附 [FFmpeg](https://ffmpeg.org/) 独立程序，许可为 LGPL-2.1-or-later；同一 Release 提供对应 FFmpeg 完整源码，包内包含许可和构建方式。

## 从源码构建

需要 Rust 1.88.0 或更新版本及平台编译工具链。仓库的 `rust-toolchain.toml` 固定使用 1.88.0。Linux 构建还需要 OpenSSL 开发库和 pkg-config。

在本仓库根目录运行：

```bash
cargo build --release -p bili-cli --locked
./target/release/bili --help
```

或者安装到 Cargo 的可执行文件目录：

```bash
cargo install --path crates/bili-cli --locked
bili --help
```

从源码安装时，合并音视频和嵌入章节需要另外安装 FFmpeg。查找顺序为 `--ffmpeg PATH` / 配置中的 `ffmpeg_path`、`bili` 同目录的 `ffmpeg`、系统 `PATH`。例如：

```bash
bili --ffmpeg /opt/homebrew/bin/ffmpeg download BV1xx411c7mD --no-extras
```

## 终端展示

默认输出中文表格与摘要，适合直接阅读；`--json` 保持单个结构化结果，供 Agent 或脚本解析。列表展示视频标题、时长、BV 号和分页，并给出下载、下一页命令。窄终端自动改为多行条目，长标题按显示宽度截断。

```bash
# 直接浏览，不加 --json
bili up videos 504018708 --page 1
bili info BV17x411w7KC
bili tasks list

# Agent / 脚本继续使用 JSON
bili up videos 504018708 --page 1 --json
```

下载时在 stderr 显示进度条、媒体字节数、分片完成速度和当前阶段；音视频下载完毕后，仍会继续显示合并或附加文件处理。Ctrl-C 后显示任务 ID 与恢复命令。重定向或 `--json` 模式使用低频纯文本进度；设置 `NO_COLOR=1` 可关闭颜色。

## 查找 UP 与视频

按昵称／关键词搜索 UP 或视频，结果包含可继续查询的 UID 或可下载的视频 ID：

```bash
bili search '影视飓风' --type user
bili search 'Rust 入门' --type video --page 1
bili search 'Rust 入门' --type video --page 2 --json
```

`--type user` 返回账号列表，不会自动选中同名账号；选择 UID 后使用 `up videos` 查看投稿。`--type video` 返回视频列表，可将 BV／av 号或结果中的 URL 交给 `info` 或 `download`；缺少有效视频 ID 的条目会标为不可下载。搜索混入的课程等非普通视频条目不会按 AV 号回退，需提供对应的完整内容 URL 查询。关键词搜索使用 B 站默认相关度排序，每次只获取指定页；表格和 JSON 输出均支持。

已知 UID 时，沿用原 GUI 的查询方式浏览投稿，再选择结果中的 BV 号交给下载命令：

```bash
# 使用 UP 的 UID；每次只返回指定页
bili up videos 946974 --page 1 --json
bili up videos 946974 --page 2 --json

# 也可提供 UP 空间 URL
bili up videos 'https://space.bilibili.com/946974' --json

# 从结果中选择 bvid，检查分 P 后下载
bili info BV1xx411c7mD --json
bili download BV1xx411c7mD --output ./downloads --no-extras --json
```

原 GUI 的搜索入口支持按视频 ID、UID、收藏夹 ID 或 URL 解析，CLI 对应提供 `search`：

```bash
bili search uid946974 --page 1 --json
bili search fid123456 --page 1 --json
bili search BV1xx411c7mD --json
bili search 'https://space.bilibili.com/946974' --page 1 --json
```

不带 `--type` 时，`search` 沿用 ID／URL 解析，接受 `uid…`、`fid…`、BV / av / ep / ss ID 和对应完整 B 站 URL；关键词搜索需要显式指定 `--type user` 或 `--type video`。为避免数字 UID 与收藏夹 ID 混淆，裸数字仅用于明确的 `up videos`、`fav list`、`fav videos` 子命令。

收藏夹浏览支持先列某个 UP 的收藏夹，再列指定收藏夹的视频：

```bash
bili fav list 946974 --json
bili fav videos 123456 --page 1 --json
bili fav videos 'https://space.bilibili.com/946974/favlist?fid=123456' --json
```

视频浏览命令只获取指定页的结果，默认第 1 页，`--page` 必须为正整数；不会自动翻页或创建下载任务。视频浏览返回 `items`、`page`、`page_size`、`total` 和 `has_more`。可下载视频条目包含 `bvid` 与 `url`，供 Agent 选择后调用 `info` / `download`。收藏夹中无法下载的条目仍保留，并标明 `downloadable: false` 及原因。

内容能否被浏览取决于 B 站接口返回、隐私设置和登录权限；可先执行 `bili auth login`。接口失败会返回稳定错误代码 `DISCOVERY_FAILED`，输入不合法返回 `INVALID_SOURCE`，命令行参数错误返回 `INVALID_ARGUMENT`。

遇到 HTTP 412 或 B 站风控响应时，CLI 返回 `BILIBILI_RISK_CONTROL`，只报告接口路径和状态码，不打印整页 HTML，也不自动重试。先用 `bili auth status` 检查会话，必要时执行 `bili auth login`；如网页要求验证，请人工完成后再试，或稍后重试。登录不保证解除风控。

## 常用下载命令

```bash
# 查看元数据；支持完整 bilibili.com URL 和 BV / av / ep / ss ID
bili info BV1xx411c7mD --json

# 下载视频；默认在前台等待全部选中任务完成
bili download 'https://www.bilibili.com/video/BV1xx411c7mD' --output ./downloads --no-extras

# 仅下载第 1、3 个分 P
bili download BV1xx411c7mD --parts 1,3 --output ./downloads --no-extras

# 分开保存音视频，跳过附加内容，无需 FFmpeg
bili download BV1xx411c7mD --no-merge --no-extras

# 仅音频
bili download BV1xx411c7mD --audio-only --no-extras

# 优先选择 1080P、AVC；最终结果报告实际画质和编码
bili download BV1xx411c7mD --quality 80 --codec AVC --no-extras --json

# 查看保存的任务与进度
bili tasks list --json
bili tasks get TASK_ID --json

# 恢复某个暂停或失败任务，并等待完成
bili resume TASK_ID --json
```

普通视频默认下载当前视频的全部分 P；URL 带 `?p=2` 时默认选择对应分 P。`ep` 默认只选择该集，`ss` 默认选择正片集数；`--parts` 按解析结果中的一基序号选择，`--all` 可扩展至当前合集。课程使用完整的 `/cheese/play/ep…` 或 `/cheese/play/ss…` URL。短链接需先展开为完整视频 URL。

| 参数 | 作用 |
| --- | --- |
| `--output DIR` / `-o DIR` | 输出根目录，随任务保存，续传沿用 |
| `--parts 1,3` | 选择分 P / 集数，序号从 1 开始 |
| `--all` | 下载解析到的整个合集，与 `--parts` 互斥 |
| `--audio-only` / `--video-only` | 只下载音频 / 视频，两者互斥 |
| `--no-merge` | 保留独立音视频文件 |
| `--no-extras` | 跳过弹幕、字幕、封面、NFO、JSON 元数据、章节和跳过标记 |
| `--quality ID` | 画质优先级；例如 `80` 为 1080P，`120` 为 4K |
| `--codec AVC/HEVC/AV1` | 编码优先级，不区分大小写 |
| `--json` | stdout 输出单个 JSON 结果；进度与诊断写入 stderr |
| `--data-dir DIR` | 配置、登录会话和任务记录目录 |
| `--ffmpeg PATH` | 指定 FFmpeg 可执行文件 |

`--quality` 和 `--codec` 表示优先选择，目标不可用时可能回退；以任务结果中的实际选择为准。最高可用画质与受限内容取决于账号权限和 B 站返回的媒体格式。

未使用 `--no-extras` 时沿用上游附加下载能力。只想要音视频文件时建议加上该参数；同时使用 `--no-merge` 可跳过 FFmpeg 处理。

## 登录

查看当前状态，或用 B 站客户端扫描终端二维码并确认：

```bash
bili auth status --json
bili auth login
bili auth login --timeout 180 --json
```

二维码、登录 URL 和等待提示写入 stderr；`--json` 的 stdout 只在结束时输出一次结果。登录成功后保存会话，不输出 Cookie 或刷新令牌。

也可从 stdin 导入原始 `SESSDATA` 值或完整 Cookie 请求头，验证成功后才保存：

```bash
bili auth set-cookie < /path/to/private-cookie.txt
```

Cookie 不作为命令行参数传递。登录会话保存在数据目录的 `config.json`，不应将该文件纳入版本管理。

## 数据目录与续传

目录选择顺序是 `--data-dir`、`BILI_DATA_DIR`、`XDG_DATA_HOME/bili`、平台默认目录。macOS 默认是 `~/Library/Application Support/bili`，其他平台有 `HOME` 时默认是 `~/.local/share/bili`。默认下载目录是数据目录中的 `视频下载`，可通过 `--output` 指定。

```bash
bili --data-dir ./state download BV1xx411c7mD --output ./downloads --no-extras
# Ctrl-C 停止下载并保存分片进度；上方命令会打印任务 ID
bili --data-dir ./state tasks list --json
bili --data-dir ./state resume TASK_ID --json
```

同一数据目录只允许一个运行下载引擎的命令，避免重复写入；不同数据目录可分别运行。`tasks list/get` 直接读取已保存记录，不启动下载，可与下载命令并行使用。其结果带 `live: false`：字节进度来自落盘记录，状态是从保存的完成情况或错误推导出的状态，不是另一个进程的即时状态。

下载命令只运行本次选择的任务；`resume` 只恢复指定任务。再次启动 CLI 不会自动运行历史任务。任务保存独立的路径和下载选项，一次命令的参数不改变其他任务的设置。

## Agent 输出与退出码

Agent 直接执行 `bili … --json`，无需常驻进程。成功结果形式为：

```json
{"ok":true,"result":{"tasks":[],"live":false}}
```

失败结果包含稳定的错误代码和说明，例如：

```json
{"ok":false,"error":{"code":"TASK_NOT_FOUND","message":"No task with ID missing-id"}}
```

下载结果还包含 `completed` 和 `tasks`；中断结果包含 `interrupted` 和已保存任务。摘要返回任务 ID、状态、已完成字节、总字节、实际画质和输出目录，不返回媒体签名 URL 或登录凭据。一次下载可以创建多个任务，stdout 会在全部选中任务结束后输出一个 JSON 文档。

| 退出码 | 含义 |
| --- | --- |
| `0` | 命令成功；下载命令表示全部选中任务完成 |
| `1` | 下载失败、目标 / 任务不存在、登录失败或其他运行错误 |
| `2` | 命令行参数错误 |
| `130` | Ctrl-C 中断；已有下载进度会保存，可按任务 ID 恢复 |

## 下载可靠性

- 音视频分片自动重试连接失败、正文断流、超时、短正文以及 HTTP 408 / 429 / 5xx。每个分片最多尝试 4 次，从 500ms 开始指数退避并加入随机延迟。
- 连接超时为 10 秒，单次分片请求和正文读取超时为 60 秒。
- 只有 HTTP 206、`Content-Range`、实际字节数均通过校验，且数据完整写入后，才标记分片完成。
- 重试耗尽时保留成功分片；`resume` 只补未完成的部分。临时文件丢失或大小不符时，对应媒体文件的分片记录会失效并重新下载。
- Ctrl-C 会停止工作任务及其 FFmpeg 子进程，保存进度后退出。

## 开发与验证

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

提交前检查敏感信息（需要开发者另行安装 Gitleaks）：

```bash
gitleaks dir --redact=100 .
gitleaks git --redact=100 --log-opts="--all" .
```

CI 同时扫描工作文件与 Git 历史，并增加 B 站登录 Cookie 规则。请勿提交真实登录配置、会话 Cookie、任务记录或带签名的媒体链接；测试应使用合成数据。`.gitignore` 已排除常见运行数据目录、配置、环境变量文件和下载的音视频。

测试包含本地 HTTP 模拟断流、超时、分片范围 / 长度校验、已下载分片保留与最终文件字节对比，以及发现命令的参数与输入验证、任务读取和 JSON 输出验证，无需 B 站账号。这些离线测试不依赖真实登录会话；线上接口行为和可用画质取决于 B 站与账号权限。

根目录工作区包含下载内核 `crates/bili-core`、命令行入口 `crates/bili-cli` 和插件接口 `crates/bili-plugin-api`。CI 执行 Rust 格式、Clippy 与测试；手动 `Build CLI artifacts` 工作流可生成当前运行平台的 CLI 构建产物，不自动发布桌面安装包或创建 Release。

## 许可与致谢

沿用 [MIT License](LICENSE)，保留上游作者 lanyeeee 的版权与署名。感谢 [上游项目](https://github.com/lanyeeee/bilibili-video-downloader)、[bilibili-API-collect](https://github.com/pskdje/bilibili-API-collect)、[FFmpeg](https://github.com/FFmpeg/FFmpeg)、[danmu2ass](https://github.com/gwy15/danmu2ass) 和 [BilibiliSponsorBlock](https://github.com/hanydd/BilibiliSponsorBlock)。

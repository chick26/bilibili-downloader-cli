# 上游来源与许可

本项目基于 [lanyeeee/bilibili-video-downloader](https://github.com/lanyeeee/bilibili-video-downloader) 整理为独立的 Rust CLI。

- 开发基线：`1254c6bf2a09a590163091e67651bfc9942171e2`。
- 基线来源：https://github.com/lanyeeee/bilibili-video-downloader/commit/1254c6bf2a09a590163091e67651bfc9942171e2 。
- 上游作者版权声明与完整 MIT 许可保留在 [LICENSE](LICENSE)。
- 投稿查询的会话兼容处理参考了上游 [PR #91](https://github.com/lanyeeee/bilibili-video-downloader/pull/91)。本 CLI 使用登录响应中的真实账号 ID。

CLI 迭代包括下载断流重试、超时与分片校验、断点进度保存、UID／收藏夹浏览、JSON 调用接口与中文终端展示。

此源码目录保留运行内核及其插件 ABI 接口，未纳入原 GUI、前端工具链、插件开发示例或预编译 FFmpeg。插件接口包仍使用原 crate 名称，以保持内核引用与接口一致。

FFmpeg 作为独立可执行文件由用户或包管理器提供，本源码目录不分发 FFmpeg 二进制。今后若提供包含 FFmpeg 的便携包，需要同时提供相应许可、构建信息与对应源码。Rust 依赖项各自的许可由相应项目规定。

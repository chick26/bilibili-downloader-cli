use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "bili",
    version,
    about = "B站视频下载器 · 投稿浏览、断点续传与 Agent 调用"
)]
#[command(
    after_help = "示例:\n  bili up videos 504018708             浏览 UP 投稿\n  bili info BV17x411w7KC               查看视频与分 P\n  bili download BV17x411w7KC -o ./videos\n  bili tasks list                     查看已保存的任务\n\n默认显示终端表格；加 --json 输出单个 JSON 结果供 Agent 使用。\n下载在前台运行，Ctrl-C 保存进度；bili resume ID 继续下载。"
)]
pub struct Cli {
    /// 任务记录与登录配置目录（也可用 BILI_DATA_DIR）
    #[arg(long, global = true, value_name = "DIR")]
    pub data_dir: Option<PathBuf>,
    /// 输出单个 JSON 结果；进度和诊断写入 stderr
    #[arg(long, global = true)]
    pub json: bool,
    /// 合并与嵌入内容所用的 FFmpeg 路径
    #[arg(long, global = true, value_name = "PATH")]
    pub ffmpeg: Option<PathBuf>,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// 通过 ID／URL 查找内容；加 --type user/video 搜索关键词
    Search(SearchArgs),
    /// 浏览 UP 投稿
    Up {
        #[command(subcommand)]
        command: UpCommand,
    },
    /// 浏览收藏夹与收藏视频
    Fav {
        #[command(subcommand)]
        command: FavCommand,
    },
    /// 查看视频信息、分 P 与剧集
    Info { source: String },
    /// 下载视频，在前台等待完成
    Download(DownloadArgs),
    /// 查看已保存的任务与进度
    Tasks {
        #[command(subcommand)]
        command: TaskCommand,
    },
    /// 恢复指定任务的下载
    Resume { id: String },
    /// 检查登录状态、扫码登录或导入 Cookie
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
}

#[derive(Debug, Args)]
pub struct PageSourceArgs {
    pub source: String,
    /// 页码，从 1 开始；仅获取指定页
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..))]
    pub page: u32,
}

#[derive(Debug, Args)]
pub struct SearchArgs {
    #[command(flatten)]
    pub source: PageSourceArgs,
    /// 关键词搜索类型：user 为 UP，video 为视频；省略时按 ID／URL 解析
    #[arg(long = "type", value_enum)]
    pub search_type: Option<SearchType>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum SearchType {
    User,
    Video,
}

#[derive(Debug, Subcommand)]
pub enum UpCommand {
    /// 通过 UID、uid123 或空间 URL 浏览投稿
    Videos(PageSourceArgs),
}

#[derive(Debug, Subcommand)]
pub enum FavCommand {
    /// 通过 UID 或空间 URL 列出有权访问的收藏夹
    List { source: String },
    /// 通过 FID、fid123 或收藏夹 URL 浏览视频
    Videos(PageSourceArgs),
}

#[derive(Debug, Args)]
pub struct DownloadArgs {
    pub source: String,
    /// 选择分 P／剧集的序号，从 1 开始，如 1,3
    #[arg(long, value_delimiter = ',', value_parser = clap::value_parser!(u32).range(1..), conflicts_with = "all")]
    pub parts: Vec<u32>,
    /// 选择当前合集中的全部视频／剧集
    #[arg(long)]
    pub all: bool,
    /// 下载目录（随任务保存，恢复时沿用）
    #[arg(short, long, value_name = "DIR")]
    pub output: Option<PathBuf>,
    /// 只下载音频
    #[arg(long, conflicts_with = "video_only")]
    pub audio_only: bool,
    /// 只下载视频
    #[arg(long)]
    pub video_only: bool,
    /// 分别保存音视频，不合并
    #[arg(long)]
    pub no_merge: bool,
    /// 首选画质 ID：6,16,32,64,74,80,100,112,116,120,125,126,127
    #[arg(long, value_name = "ID")]
    pub quality: Option<i64>,
    /// 首选视频编码
    #[arg(long, value_enum, ignore_case = true)]
    pub codec: Option<Codec>,
    /// 跳过弹幕、字幕、封面、元数据和章节等附加内容
    #[arg(long)]
    pub no_extras: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Codec {
    #[value(name = "AVC")]
    Avc,
    #[value(name = "HEVC")]
    Hevc,
    #[value(name = "AV1")]
    Av1,
}

#[derive(Debug, Subcommand)]
pub enum TaskCommand {
    /// 列出全部已保存任务
    List,
    /// 查看单个任务的详情和恢复命令
    Get { id: String },
}

#[derive(Debug, Subcommand)]
pub enum AuthCommand {
    /// 查看当前账号登录状态
    Status,
    /// 显示终端二维码并等待确认登录
    Login {
        /// 最多等待的秒数
        #[arg(long, default_value_t = 180, value_parser = clap::value_parser!(u64).range(1..))]
        timeout: u64,
    },
    /// 从标准输入读取 SESSDATA 或 Cookie，校验后保存
    SetCookie,
}

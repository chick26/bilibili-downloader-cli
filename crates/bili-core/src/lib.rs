//! Headless Bilibili API client and resumable download engine.

pub mod api_error;
pub mod bili_client;
pub mod config;
pub mod danmaku_xml_to_ass;
pub mod discovery;
pub mod downloader;
pub mod errors;
pub mod events;
pub mod extensions;
pub mod keyword_search;
pub mod media_transfer;
pub mod plugin;
pub mod runtime;
pub mod service;
pub mod task_options;
pub mod types;
pub mod utils;
pub mod wbi;
#[allow(warnings)]
pub mod protobuf {
    include!("./bilibili.community.service.dm.v1.rs");
}

pub use runtime::{AppContext, Runtime};

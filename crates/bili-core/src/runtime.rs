use std::{
    fs::{File, OpenOptions},
    ops::Deref,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock, Weak},
};

use eyre::{WrapErr, eyre};
use fs4::fs_std::FileExt;
use parking_lot::RwLock;
use tokio::sync::broadcast;

use crate::{
    bili_client::BiliClient, config::Config, downloader::download_manager::DownloadManager,
    events::CoreEvent, plugin::plugin_manager::PluginManager,
};

/// Owns the engine and the exclusive lock on its persisted queue.
///
/// Create it inside a Tokio runtime and keep it alive for the duration of a command.
/// Creating the runtime does not restore or start any downloads.
pub struct Runtime {
    services: Arc<Services>,
    _data_lock: File,
}

struct Shared {
    data_dir: PathBuf,
    config: RwLock<Config>,
    events: broadcast::Sender<CoreEvent>,
    package: PackageInfo,
}

struct Services {
    shared: Arc<Shared>,
    bili_client: OnceLock<Arc<BiliClient>>,
    download_manager: OnceLock<Arc<DownloadManager>>,
    plugin_manager: OnceLock<Arc<PluginManager>>,
}

/// A portable engine context. Service references are weak to avoid ownership cycles.
#[derive(Clone)]
pub struct AppContext {
    shared: Arc<Shared>,
    services: Weak<Services>,
}

pub struct PackageInfo {
    pub version: &'static str,
}

pub struct AppPaths<'a> {
    data_dir: &'a Path,
}

impl AppPaths<'_> {
    pub fn app_data_dir(&self) -> eyre::Result<PathBuf> {
        Ok(self.data_dir.to_path_buf())
    }
}

/// A borrowed state reference whose lock guards borrow the context, not a temporary.
pub struct BorrowedState<'a, T>(&'a T);

impl<'a, T> BorrowedState<'a, T> {
    pub fn inner(&self) -> &'a T {
        self.0
    }
}

impl<T> Deref for BorrowedState<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.0
    }
}

impl<'a, T> BorrowedState<'a, RwLock<T>> {
    pub fn read(&self) -> parking_lot::RwLockReadGuard<'a, T> {
        self.0.read()
    }

    pub fn write(&self) -> parking_lot::RwLockWriteGuard<'a, T> {
        self.0.write()
    }
}

impl Runtime {
    pub fn load(data_dir: impl Into<PathBuf>) -> eyre::Result<Self> {
        let data_dir = data_dir.into();
        let config = Config::load(&data_dir)?;
        Self::new(data_dir, config)
    }

    pub fn new(data_dir: impl Into<PathBuf>, config: Config) -> eyre::Result<Self> {
        if config.task_concurrency == 0 || config.chunk_concurrency == 0 {
            return Err(eyre!("任务和分片并发数必须大于 0"));
        }
        tokio::runtime::Handle::try_current().wrap_err("创建下载引擎需要 Tokio runtime")?;

        let data_dir = data_dir.into();
        std::fs::create_dir_all(&data_dir)
            .wrap_err_with(|| format!("创建数据目录失败: {}", data_dir.display()))?;
        let data_lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(data_dir.join(".lock"))
            .wrap_err("打开数据目录锁失败")?;
        if !data_lock
            .try_lock_exclusive()
            .wrap_err("获取数据目录锁失败")?
        {
            return Err(eyre!("已有命令使用此 data-dir: {}", data_dir.display()));
        }

        let (events, _) = broadcast::channel(256);
        let services = Arc::new(Services {
            shared: Arc::new(Shared {
                data_dir,
                config: RwLock::new(config),
                events,
                package: PackageInfo {
                    version: env!("CARGO_PKG_VERSION"),
                },
            }),
            bili_client: OnceLock::new(),
            download_manager: OnceLock::new(),
            plugin_manager: OnceLock::new(),
        });
        let runtime = Self {
            services,
            _data_lock: data_lock,
        };
        let context = runtime.context();

        let _ = runtime
            .services
            .bili_client
            .set(Arc::new(BiliClient::new(context.clone())));
        let _ = runtime
            .services
            .download_manager
            .set(Arc::new(DownloadManager::new(context.clone())));
        let _ = runtime
            .services
            .plugin_manager
            .set(Arc::new(PluginManager::new(&context)?));
        Ok(runtime)
    }

    pub fn context(&self) -> AppContext {
        AppContext {
            shared: self.services.shared.clone(),
            services: Arc::downgrade(&self.services),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<CoreEvent> {
        self.services.shared.events.subscribe()
    }

    pub fn flush_progress(&self) -> eyre::Result<()> {
        self.context().get_download_manager().flush_progress()
    }

    /// Stops the workers before the directory lock is released.
    pub async fn shutdown(&self) -> eyre::Result<()> {
        self.context().get_download_manager().shutdown().await
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        if let Some(manager) = self.services.download_manager.get() {
            for task in manager.download_tasks.read().values() {
                let _ = task.cancel_sender.send(());
            }
        }
    }
}

impl AppContext {
    /// Gives a task its own configuration snapshot while retaining the shared services.
    pub fn with_config(&self, config: Config) -> Self {
        Self {
            shared: Arc::new(Shared {
                data_dir: self.shared.data_dir.clone(),
                config: RwLock::new(config),
                events: self.shared.events.clone(),
                package: PackageInfo {
                    version: self.shared.package.version,
                },
            }),
            services: self.services.clone(),
        }
    }

    pub fn path(&self) -> AppPaths<'_> {
        AppPaths {
            data_dir: &self.shared.data_dir,
        }
    }

    pub fn package_info(&self) -> &PackageInfo {
        &self.shared.package
    }

    pub fn is_active(&self) -> bool {
        self.services.strong_count() > 0
    }

    pub fn get_config(&self) -> BorrowedState<'_, RwLock<Config>> {
        BorrowedState(&self.shared.config)
    }

    pub fn get_bili_client(&self) -> Arc<BiliClient> {
        self.services()
            .bili_client
            .get()
            .expect("BiliClient is initialized before use")
            .clone()
    }

    pub fn get_download_manager(&self) -> Arc<DownloadManager> {
        self.services()
            .download_manager
            .get()
            .expect("DownloadManager is initialized before use")
            .clone()
    }

    pub fn get_plugin_manager(&self) -> Arc<PluginManager> {
        self.services()
            .plugin_manager
            .get()
            .expect("PluginManager is initialized before use")
            .clone()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<CoreEvent> {
        self.shared.events.subscribe()
    }

    pub(crate) fn emit(&self, event: CoreEvent) {
        // An engine without an event subscriber still performs downloads normally.
        let _ = self.shared.events.send(event);
    }

    fn services(&self) -> Arc<Services> {
        self.services
            .upgrade()
            .expect("Runtime must remain alive while its context is used")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn data_directory_has_one_owner_and_unlocks_on_drop() {
        let dir = std::env::temp_dir().join(format!("bili-runtime-{}", uuid::Uuid::new_v4()));
        let runtime = Runtime::load(&dir).unwrap();
        let context = runtime.context();
        assert!(context.is_active());
        let second = Runtime::load(&dir);
        assert!(second.is_err());
        assert!(format!("{:?}", second.err().unwrap()).contains("已有命令使用此 data-dir"));
        drop(runtime);
        assert!(!context.is_active());
        let replacement = Runtime::load(&dir).unwrap();
        drop(replacement);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

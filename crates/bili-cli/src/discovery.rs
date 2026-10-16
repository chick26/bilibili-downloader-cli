use bili_core::{
    bili_client::BiliClient,
    discovery::{DiscoveryTarget, discover, favorite_folders},
};
use serde_json::json;

use crate::{
    args::{Command, FavCommand, UpCommand},
    output::{CliError, Outcome},
};

/// Validate discovery sources before creating a runtime, opening task records,
/// or making a network request.
pub enum Request {
    Search { target: DiscoveryTarget, page: u32 },
    FavoriteFolders { uid: i64 },
}

impl Request {
    pub fn from_command(command: &Command) -> Result<Option<Self>, CliError> {
        let request = match command {
            Command::Search(args) => Self::Search {
                target: DiscoveryTarget::parse(&args.source)
                    .map_err(|err| CliError::report("INVALID_SOURCE", err))?,
                page: args.page,
            },
            Command::Up {
                command: UpCommand::Videos(args),
            } => Self::Search {
                target: DiscoveryTarget::parse_up(&args.source)
                    .map_err(|err| CliError::report("INVALID_SOURCE", err))?,
                page: args.page,
            },
            Command::Fav {
                command: FavCommand::List { source },
            } => {
                let DiscoveryTarget::Up { uid } = DiscoveryTarget::parse_up(source)
                    .map_err(|err| CliError::report("INVALID_SOURCE", err))?
                else {
                    unreachable!("parse_up only returns Up targets")
                };
                Self::FavoriteFolders { uid }
            }
            Command::Fav {
                command: FavCommand::Videos(args),
            } => Self::Search {
                target: DiscoveryTarget::parse_favorite(&args.source)
                    .map_err(|err| CliError::report("INVALID_SOURCE", err))?,
                page: args.page,
            },
            _ => return Ok(None),
        };
        Ok(Some(request))
    }

    pub async fn execute(&self, client: &BiliClient) -> Result<Outcome, CliError> {
        let result = match self {
            Self::Search { target, page } => json!(
                discover(client, target, *page)
                    .await
                    .map_err(|err| CliError::report("DISCOVERY_FAILED", err))?
            ),
            Self::FavoriteFolders { uid } => json!(
                favorite_folders(client, *uid)
                    .await
                    .map_err(|err| CliError::report("DISCOVERY_FAILED", err))?
            ),
        };
        Ok(Outcome::success(result))
    }
}

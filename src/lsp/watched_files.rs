use std::path::Path;

use lsp_server::{Connection, Request, RequestId};
use lsp_types::{
    DidChangeWatchedFilesClientCapabilities, DidChangeWatchedFilesRegistrationOptions,
    FileSystemWatcher, OneOf, Registration, RegistrationParams, RelativePattern,
    notification::{DidChangeWatchedFiles, Notification as _},
    request::{RegisterCapability, Request as _},
};
use serde_json::to_value;
use url::Url;

use super::ServerResult;

pub(super) fn register<'a>(
    connection: &Connection,
    capabilities: DidChangeWatchedFilesClientCapabilities,
    sources: impl Iterator<Item = &'a Path>,
) -> ServerResult<Option<RequestId>> {
    if capabilities.dynamic_registration != Some(true) {
        return Ok(None);
    }
    let mut watchers = Vec::new();
    for path in sources {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.contains(['*', '?', '[', ']', '{', '}', '\\']))
            .unwrap_or("*");
        let base_uri = if capabilities.relative_pattern_support == Some(true) {
            path.parent()
                .and_then(|parent| Url::from_directory_path(parent).ok())
                .and_then(|url| url.as_str().parse().ok())
        } else {
            None
        };
        let glob_pattern = match base_uri {
            Some(uri) => RelativePattern {
                base_uri: OneOf::Right(uri),
                pattern: name.to_owned(),
            }
            .into(),
            None => format!("**/{name}").into(),
        };
        watchers.push(FileSystemWatcher {
            glob_pattern,
            kind: None,
        });
    }
    if watchers.is_empty() {
        return Ok(None);
    }
    watchers.sort_unstable();
    watchers.dedup();
    let params = RegistrationParams {
        registrations: vec![Registration {
            id: "oxel.projectSources".into(),
            method: DidChangeWatchedFiles::METHOD.into(),
            register_options: Some(to_value(DidChangeWatchedFilesRegistrationOptions {
                watchers,
            })?),
        }],
    };
    let id = RequestId::from("oxel.registerProjectSources".to_owned());
    let request = Request::new(id.clone(), RegisterCapability::METHOD.into(), params);
    connection.sender.send(request.into())?;
    Ok(Some(id))
}

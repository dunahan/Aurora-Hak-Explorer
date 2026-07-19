//! KDE's archive-extraction drag protocol.
//!
//! KIO deliberately avoids receiving enormous `text/uri-list` payloads from
//! archive applications.  Ark instead advertises a small D-Bus endpoint and
//! KIO asks that endpoint to extract the current selection into the drop
//! directory.  Offering the same protocol keeps very large AHE drags reliable
//! while the ordinary URI-list target remains available to other desktops.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};

const OBJECT_PATH: &str = "/DndExtract/1";

#[derive(Clone, Default)]
struct ExtractService {
    paths: Arc<RwLock<Vec<PathBuf>>>,
}

#[zbus::interface(name = "org.kde.ark.DndExtract")]
impl ExtractService {
    #[zbus(name = "extractSelectedFilesTo")]
    fn extract_selected_files_to(&self, destination: &str) -> zbus::fdo::Result<()> {
        let destination = Path::new(destination);
        if !destination.is_dir() {
            return Err(zbus::fdo::Error::Failed(format!(
                "The drop destination is not a local directory: {}",
                destination.display()
            )));
        }

        let paths = self
            .paths
            .read()
            .map_err(|_| zbus::fdo::Error::Failed("The drag selection is unavailable".into()))?
            .clone();
        if paths.is_empty() {
            return Err(zbus::fdo::Error::Failed(
                "The drag selection is empty".into(),
            ));
        }

        for source in paths {
            let filename = source.file_name().ok_or_else(|| {
                zbus::fdo::Error::Failed(format!(
                    "The staged resource has no filename: {}",
                    source.display()
                ))
            })?;
            let target = destination.join(filename);
            fs::copy(&source, &target).map_err(|error| {
                zbus::fdo::Error::Failed(format!(
                    "Could not copy {} to {}: {error}",
                    source.display(),
                    target.display()
                ))
            })?;
        }
        Ok(())
    }
}

pub struct Bridge {
    _connection: zbus::blocking::Connection,
    service: String,
    paths: Arc<RwLock<Vec<PathBuf>>>,
}

impl Bridge {
    pub fn new() -> Result<Self, String> {
        let connection =
            zbus::blocking::Connection::session().map_err(|error| error.to_string())?;
        let service = connection
            .unique_name()
            .ok_or_else(|| "The D-Bus session did not assign a unique service name".to_owned())?
            .to_string();
        let interface = ExtractService::default();
        let paths = Arc::clone(&interface.paths);
        connection
            .object_server()
            .at(OBJECT_PATH, interface)
            .map_err(|error| error.to_string())?;
        Ok(Self {
            _connection: connection,
            service,
            paths,
        })
    }

    pub fn set_paths(&self, paths: Vec<PathBuf>) {
        if let Ok(mut current) = self.paths.write() {
            *current = paths;
        }
    }

    pub fn service(&self) -> &str {
        &self.service
    }

    pub fn path(&self) -> &'static str {
        OBJECT_PATH
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kde_extract_request_copies_the_entire_offer() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let first = source.path().join("first.mdl");
        let second = source.path().join("second.dds");
        fs::write(&first, b"model").unwrap();
        fs::write(&second, b"texture").unwrap();

        let bridge = Bridge::new().unwrap();
        bridge.set_paths(vec![first, second]);
        let caller = zbus::blocking::Connection::session().unwrap();
        let proxy = zbus::blocking::Proxy::new(
            &caller,
            bridge.service(),
            bridge.path(),
            "org.kde.ark.DndExtract",
        )
        .unwrap();
        let destination = destination.path().to_string_lossy().into_owned();
        proxy
            .call_method("extractSelectedFilesTo", &(destination.as_str(),))
            .unwrap();

        assert_eq!(
            fs::read(Path::new(&destination).join("first.mdl")).unwrap(),
            b"model"
        );
        assert_eq!(
            fs::read(Path::new(&destination).join("second.dds")).unwrap(),
            b"texture"
        );
    }
}

use std::io;
use std::os::raw::*;
use std::path::{Path, PathBuf};
use std::str::Utf8Error;
use std::sync::Arc;

use percent_encoding::percent_decode;
use x11rb::protocol::xproto::{self, ConnectionExt};

use super::atoms::AtomName::None as DndNone;
use super::atoms::*;
use super::{util, CookieResultExt, X11Error, XConnection};

#[derive(Debug, Clone, Copy)]
pub enum DndState {
    Accepted,
    Rejected,
}

#[derive(Debug)]
pub enum DndDataParseError {
    EmptyData,
    InvalidUtf8(#[allow(dead_code)] Utf8Error),
    HostnameSpecified(#[allow(dead_code)] String),
    UnexpectedProtocol(#[allow(dead_code)] String),
    UnresolvablePath(#[allow(dead_code)] io::Error),
    DataTooLarge,
}

impl From<Utf8Error> for DndDataParseError {
    fn from(e: Utf8Error) -> Self {
        DndDataParseError::InvalidUtf8(e)
    }
}

impl From<io::Error> for DndDataParseError {
    fn from(e: io::Error) -> Self {
        DndDataParseError::UnresolvablePath(e)
    }
}

pub struct Dnd {
    xconn: Arc<XConnection>,
    // Populated by XdndEnter event handler
    pub version: Option<c_long>,
    pub type_list: Option<Vec<xproto::Atom>>,
    // Populated by XdndPosition event handler
    pub source_window: Option<xproto::Window>,
    // Populated by SelectionNotify event handler (triggered by XdndPosition event handler)
    pub result: Option<Result<Vec<PathBuf>, DndDataParseError>>,
    pub incremental: Option<IncrementalTransfer>,
    pub drop_requested: bool,
    pub selection_requested: bool,
}

pub struct IncrementalTransfer {
    window: xproto::Window,
    data: Vec<c_uchar>,
}

const MAX_INCREMENTAL_DND_BYTES: usize = 128 * 1024 * 1024;

impl Dnd {
    pub fn new(xconn: Arc<XConnection>) -> Result<Self, X11Error> {
        Ok(Dnd {
            xconn,
            version: None,
            type_list: None,
            source_window: None,
            result: None,
            incremental: None,
            drop_requested: false,
            selection_requested: false,
        })
    }

    pub fn reset(&mut self) {
        self.version = None;
        self.type_list = None;
        self.source_window = None;
        self.result = None;
        self.incremental = None;
        self.drop_requested = false;
        self.selection_requested = false;
    }

    pub unsafe fn send_status(
        &self,
        this_window: xproto::Window,
        target_window: xproto::Window,
        state: DndState,
    ) -> Result<(), X11Error> {
        let atoms = self.xconn.atoms();
        let (accepted, action) = match state {
            DndState::Accepted => (1, atoms[XdndActionPrivate]),
            DndState::Rejected => (0, atoms[DndNone]),
        };
        self.xconn
            .send_client_msg(
                target_window,
                target_window,
                atoms[XdndStatus] as _,
                None,
                [this_window, accepted, 0, 0, action as _],
            )?
            .ignore_error();

        Ok(())
    }

    pub unsafe fn send_finished(
        &self,
        this_window: xproto::Window,
        target_window: xproto::Window,
        state: DndState,
    ) -> Result<(), X11Error> {
        let atoms = self.xconn.atoms();
        let (accepted, action) = match state {
            DndState::Accepted => (1, atoms[XdndActionPrivate]),
            DndState::Rejected => (0, atoms[DndNone]),
        };
        self.xconn
            .send_client_msg(
                target_window,
                target_window,
                atoms[XdndFinished] as _,
                None,
                [this_window, accepted, action as _, 0, 0],
            )?
            .ignore_error();

        Ok(())
    }

    pub unsafe fn get_type_list(
        &self,
        source_window: xproto::Window,
    ) -> Result<Vec<xproto::Atom>, util::GetPropertyError> {
        let atoms = self.xconn.atoms();
        self.xconn.get_property(
            source_window,
            atoms[XdndTypeList],
            xproto::Atom::from(xproto::AtomEnum::ATOM),
        )
    }

    pub unsafe fn convert_selection(&mut self, window: xproto::Window, time: xproto::Timestamp) {
        let atoms = self.xconn.atoms();
        self.selection_requested = true;
        self.xconn
            .xcb_connection()
            .convert_selection(
                window,
                atoms[XdndSelection],
                atoms[TextUriList],
                atoms[XdndSelection],
                time,
            )
            .expect_then_ignore_error("Failed to send XdndSelection event")
    }

    pub unsafe fn read_data(
        &self,
        window: xproto::Window,
    ) -> Result<Vec<c_uchar>, util::GetPropertyError> {
        let atoms = self.xconn.atoms();
        self.xconn
            .get_property(window, atoms[XdndSelection], atoms[TextUriList])
    }

    pub unsafe fn begin_incremental(&mut self, window: xproto::Window) {
        let atoms = self.xconn.atoms();
        self.incremental = Some(IncrementalTransfer {
            window,
            data: Vec::new(),
        });
        self.xconn
            .xcb_connection()
            .delete_property(window, atoms[XdndSelection])
            .expect_then_ignore_error("Failed to start incremental XDND transfer");
    }

    pub unsafe fn read_incremental_chunk(
        &mut self,
        window: xproto::Window,
    ) -> Result<Option<Result<Vec<PathBuf>, DndDataParseError>>, util::GetPropertyError> {
        let Some(transfer) = self.incremental.as_mut() else {
            return Ok(None);
        };
        if transfer.window != window {
            return Ok(None);
        }

        let atoms = self.xconn.atoms();
        let reply = self
            .xconn
            .xcb_connection()
            .get_property(
                true,
                window,
                atoms[XdndSelection],
                atoms[TextUriList],
                0,
                u32::MAX,
            )?
            .reply()?;
        if reply.type_ != atoms[TextUriList] {
            return Err(util::GetPropertyError::TypeMismatch(reply.type_));
        }
        if reply.format != 8 {
            return Err(util::GetPropertyError::FormatMismatch(reply.format.into()));
        }

        if reply.value.is_empty() {
            let mut data = std::mem::take(&mut transfer.data);
            self.incremental = None;
            return Ok(Some(self.parse_data(&mut data)));
        }
        if append_incremental_data(&mut transfer.data, &reply.value).is_err() {
            self.incremental = None;
            return Ok(Some(Err(DndDataParseError::DataTooLarge)));
        }
        Ok(None)
    }

    pub fn parse_data(&self, data: &mut [c_uchar]) -> Result<Vec<PathBuf>, DndDataParseError> {
        if !data.is_empty() {
            let mut path_list = Vec::new();
            let decoded = percent_decode(data).decode_utf8()?.into_owned();
            for uri in decoded.split("\r\n").filter(|u| !u.is_empty()) {
                // The format is specified as protocol://host/path
                // However, it's typically simply protocol:///path
                let path_str = if uri.starts_with("file://") {
                    let path_str = uri.replace("file://", "");
                    if !path_str.starts_with('/') {
                        // A hostname is specified
                        // Supporting this case is beyond the scope of my mental health
                        return Err(DndDataParseError::HostnameSpecified(path_str));
                    }
                    path_str
                } else {
                    // Only the file protocol is supported
                    return Err(DndDataParseError::UnexpectedProtocol(uri.to_owned()));
                };

                let path = Path::new(&path_str).canonicalize()?;
                path_list.push(path);
            }
            Ok(path_list)
        } else {
            Err(DndDataParseError::EmptyData)
        }
    }
}

fn append_incremental_data(
    data: &mut Vec<c_uchar>,
    chunk: &[c_uchar],
) -> Result<(), DndDataParseError> {
    if data.len().saturating_add(chunk.len()) > MAX_INCREMENTAL_DND_BYTES {
        return Err(DndDataParseError::DataTooLarge);
    }
    data.extend_from_slice(chunk);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incremental_buffer_accepts_a_fifty_thousand_file_uri_list() {
        let line = b"file:///tmp\r\n";
        let mut payload = Vec::new();
        for _ in 0..50_000 {
            append_incremental_data(&mut payload, line).unwrap();
        }
        assert_eq!(payload.len(), line.len() * 50_000);
        assert!(payload.len() < MAX_INCREMENTAL_DND_BYTES);
    }

    #[test]
    fn incremental_buffer_rejects_unbounded_payloads() {
        let mut payload = vec![0; MAX_INCREMENTAL_DND_BYTES];
        assert!(append_incremental_data(&mut payload, b"x").is_err());
    }
}

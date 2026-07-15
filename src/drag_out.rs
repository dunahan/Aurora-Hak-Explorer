//! Native X11 file drag source used by the AppImage build.
//!
//! eframe/winit receives file drops but does not currently expose an outgoing
//! data-drag API on Linux.  This small XDND source owns a helper X11 window,
//! advertises `text/uri-list`, and serves the exported temporary files when a
//! desktop or file manager requests them.

use std::{
    os::unix::ffi::OsStrExt,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

use tempfile::TempDir;
use x11rb::{
    COPY_DEPTH_FROM_PARENT, COPY_FROM_PARENT, CURRENT_TIME, NONE,
    connection::Connection,
    protocol::{
        Event,
        xproto::{
            Atom, AtomEnum, ButtonReleaseEvent, ClientMessageData, ClientMessageEvent,
            ConnectionExt, CreateWindowAux, EventMask, GrabMode, GrabStatus, PropMode,
            SELECTION_NOTIFY_EVENT, SelectionNotifyEvent, SelectionRequestEvent, Window,
            WindowClass,
        },
    },
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};

#[derive(Clone, Copy)]
struct Atoms {
    xdnd_aware: Atom,
    xdnd_enter: Atom,
    xdnd_leave: Atom,
    xdnd_position: Atom,
    xdnd_status: Atom,
    xdnd_drop: Atom,
    xdnd_finished: Atom,
    xdnd_selection: Atom,
    xdnd_action_copy: Atom,
    text_uri_list: Atom,
    targets: Atom,
    utf8_string: Atom,
}

impl Atoms {
    fn new(connection: &RustConnection) -> Result<Self, String> {
        Ok(Self {
            xdnd_aware: atom(connection, b"XdndAware")?,
            xdnd_enter: atom(connection, b"XdndEnter")?,
            xdnd_leave: atom(connection, b"XdndLeave")?,
            xdnd_position: atom(connection, b"XdndPosition")?,
            xdnd_status: atom(connection, b"XdndStatus")?,
            xdnd_drop: atom(connection, b"XdndDrop")?,
            xdnd_finished: atom(connection, b"XdndFinished")?,
            xdnd_selection: atom(connection, b"XdndSelection")?,
            xdnd_action_copy: atom(connection, b"XdndActionCopy")?,
            text_uri_list: atom(connection, b"text/uri-list")?,
            targets: atom(connection, b"TARGETS")?,
            utf8_string: atom(connection, b"UTF8_STRING")?,
        })
    }
}

fn atom(connection: &RustConnection, name: &[u8]) -> Result<Atom, String> {
    connection
        .intern_atom(false, name)
        .map_err(|error| error.to_string())?
        .reply()
        .map(|reply| reply.atom)
        .map_err(|error| error.to_string())
}

/// Release winit's implicit pointer grab so the helper XDND connection can
/// take ownership of the remainder of the current mouse drag.
pub fn release_pointer_grab(frame: &eframe::Frame) {
    use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};

    let Ok(display_handle) = frame.display_handle() else {
        return;
    };
    let RawDisplayHandle::Xlib(handle) = display_handle.as_raw() else {
        return;
    };
    let Some(display) = handle.display else {
        return;
    };
    let Ok(xlib) = x11_dl::xlib::Xlib::open() else {
        return;
    };
    // Use winit's own Xlib connection: implicit pointer grabs are scoped to
    // the X client connection that received the original button press.
    unsafe {
        let display = display.as_ptr().cast::<x11_dl::xlib::Display>();
        (xlib.XUngrabPointer)(display, CURRENT_TIME.into());
        (xlib.XFlush)(display);
    }
}

pub fn start(_frame: &eframe::Frame, paths: Vec<PathBuf>, temporary_directory: TempDir) {
    thread::spawn(move || {
        if let Err(error) = run(paths) {
            eprintln!("Could not start outgoing file drag: {error}");
        } else {
            // File managers commonly acknowledge XDND before their copy job
            // opens the source URI. Keep the exported files alive while that
            // asynchronous job starts; dropping TempDir afterwards cleans up.
            thread::sleep(Duration::from_secs(300));
        }
        drop(temporary_directory);
    });
}

fn run(paths: Vec<PathBuf>) -> Result<(), String> {
    let (connection, screen_number) = x11rb::connect(None).map_err(|error| error.to_string())?;
    let root = connection.setup().roots[screen_number].root;
    let source = connection
        .generate_id()
        .map_err(|error| error.to_string())?;
    let atoms = Atoms::new(&connection)?;

    connection
        .create_window(
            COPY_DEPTH_FROM_PARENT,
            source,
            root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_ONLY,
            COPY_FROM_PARENT,
            &CreateWindowAux::new().event_mask(
                EventMask::BUTTON_RELEASE
                    | EventMask::BUTTON_MOTION
                    | EventMask::POINTER_MOTION
                    | EventMask::PROPERTY_CHANGE,
            ),
        )
        .map_err(|error| error.to_string())?;
    connection
        .change_property32(
            PropMode::REPLACE,
            source,
            atoms.xdnd_aware,
            AtomEnum::ATOM,
            &[5],
        )
        .map_err(|error| error.to_string())?;
    connection
        .set_selection_owner(source, atoms.xdnd_selection, CURRENT_TIME)
        .map_err(|error| error.to_string())?;
    connection.flush().map_err(|error| error.to_string())?;

    let mut grabbed = false;
    for _ in 0..20 {
        let status = connection
            .grab_pointer(
                false,
                root,
                EventMask::BUTTON_RELEASE | EventMask::BUTTON_MOTION | EventMask::POINTER_MOTION,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
                NONE,
                NONE,
                CURRENT_TIME,
            )
            .map_err(|error| error.to_string())?
            .reply()
            .map_err(|error| error.to_string())?
            .status;
        if status == GrabStatus::SUCCESS {
            grabbed = true;
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    if !grabbed {
        return Err("the desktop would not release the pointer grab".into());
    }

    let uri_list = uri_list(&paths);
    let mut target = NONE;
    let mut accepted = false;
    let mut last_time = CURRENT_TIME;

    loop {
        let event = connection
            .wait_for_event()
            .map_err(|error| error.to_string())?;
        match event {
            Event::MotionNotify(event) => {
                last_time = event.time;
                let next = find_target(&connection, root, atoms.xdnd_aware)?;
                if next != target {
                    if target != NONE {
                        send_message(&connection, target, atoms.xdnd_leave, [source, 0, 0, 0, 0])?;
                    }
                    target = next;
                    accepted = false;
                    if target != NONE {
                        send_message(
                            &connection,
                            target,
                            atoms.xdnd_enter,
                            [source, 5 << 24, atoms.text_uri_list, 0, 0],
                        )?;
                    }
                }
                if target != NONE {
                    let coordinates =
                        ((event.root_x as u16 as u32) << 16) | event.root_y as u16 as u32;
                    send_message(
                        &connection,
                        target,
                        atoms.xdnd_position,
                        [source, 0, coordinates, event.time, atoms.xdnd_action_copy],
                    )?;
                }
                connection.flush().map_err(|error| error.to_string())?;
            }
            Event::ClientMessage(event) if event.type_ == atoms.xdnd_status => {
                let data = event.data.as_data32();
                accepted = data[1] & 1 != 0;
            }
            Event::ButtonRelease(event) => {
                finish_drag(
                    &connection,
                    source,
                    target,
                    accepted,
                    event,
                    last_time,
                    atoms,
                    &uri_list,
                )?;
                break;
            }
            Event::SelectionRequest(event) => {
                serve_selection(&connection, event, atoms, &uri_list)?;
            }
            _ => {}
        }
    }

    let _ = connection.ungrab_pointer(CURRENT_TIME);
    let _ = connection.set_selection_owner(NONE, atoms.xdnd_selection, CURRENT_TIME);
    let _ = connection.destroy_window(source);
    let _ = connection.flush();
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn finish_drag(
    connection: &RustConnection,
    source: Window,
    target: Window,
    accepted: bool,
    event: ButtonReleaseEvent,
    last_time: u32,
    atoms: Atoms,
    uri_list: &[u8],
) -> Result<(), String> {
    if target == NONE || !accepted {
        if target != NONE {
            send_message(connection, target, atoms.xdnd_leave, [source, 0, 0, 0, 0])?;
        }
        return Ok(());
    }
    let time = if event.time == CURRENT_TIME {
        last_time
    } else {
        event.time
    };
    send_message(connection, target, atoms.xdnd_drop, [source, 0, time, 0, 0])?;
    connection.flush().map_err(|error| error.to_string())?;

    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        match connection
            .poll_for_event()
            .map_err(|error| error.to_string())?
        {
            Some(Event::SelectionRequest(event)) => {
                serve_selection(connection, event, atoms, uri_list)?;
            }
            Some(Event::ClientMessage(event)) if event.type_ == atoms.xdnd_finished => break,
            Some(_) => {}
            None => thread::sleep(Duration::from_millis(5)),
        }
    }
    Ok(())
}

fn serve_selection(
    connection: &RustConnection,
    event: SelectionRequestEvent,
    atoms: Atoms,
    uri_list: &[u8],
) -> Result<(), String> {
    let property = if event.property == NONE {
        event.target
    } else {
        event.property
    };
    let supported = if event.target == atoms.targets {
        connection
            .change_property32(
                PropMode::REPLACE,
                event.requestor,
                property,
                AtomEnum::ATOM,
                &[
                    atoms.targets,
                    atoms.text_uri_list,
                    atoms.utf8_string,
                    AtomEnum::STRING.into(),
                ],
            )
            .map_err(|error| error.to_string())?;
        true
    } else if event.target == atoms.text_uri_list
        || event.target == atoms.utf8_string
        || event.target == AtomEnum::STRING.into()
    {
        connection
            .change_property8(
                PropMode::REPLACE,
                event.requestor,
                property,
                event.target,
                uri_list,
            )
            .map_err(|error| error.to_string())?;
        true
    } else {
        false
    };
    let notify = SelectionNotifyEvent {
        response_type: SELECTION_NOTIFY_EVENT,
        sequence: 0,
        time: event.time,
        requestor: event.requestor,
        selection: event.selection,
        target: event.target,
        property: if supported { property } else { NONE },
    };
    connection
        .send_event(false, event.requestor, EventMask::NO_EVENT, notify)
        .map_err(|error| error.to_string())?;
    connection.flush().map_err(|error| error.to_string())?;
    Ok(())
}

fn find_target(
    connection: &RustConnection,
    root: Window,
    xdnd_aware: Atom,
) -> Result<Window, String> {
    let mut window = root;
    let mut chain = Vec::new();
    loop {
        let pointer = connection
            .query_pointer(window)
            .map_err(|error| error.to_string())?
            .reply()
            .map_err(|error| error.to_string())?;
        if pointer.child == NONE || pointer.child == window {
            break;
        }
        window = pointer.child;
        chain.push(window);
    }
    for candidate in chain.into_iter().rev() {
        let property = connection
            .get_property(false, candidate, xdnd_aware, AtomEnum::ATOM, 0, 1)
            .map_err(|error| error.to_string())?
            .reply()
            .map_err(|error| error.to_string())?;
        if property.format == 32 && property.value_len > 0 {
            return Ok(candidate);
        }
    }
    Ok(NONE)
}

fn send_message(
    connection: &RustConnection,
    destination: Window,
    message_type: Atom,
    data: [u32; 5],
) -> Result<(), String> {
    let event =
        ClientMessageEvent::new(32, destination, message_type, ClientMessageData::from(data));
    connection
        .send_event(false, destination, EventMask::NO_EVENT, event)
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn uri_list(paths: &[PathBuf]) -> Vec<u8> {
    let mut result = String::new();
    for path in paths {
        result.push_str("file://");
        for &byte in path.as_os_str().as_bytes() {
            if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
                result.push(byte as char);
            } else {
                result.push_str(&format!("%{byte:02X}"));
            }
        }
        result.push_str("\r\n");
    }
    result.into_bytes()
}

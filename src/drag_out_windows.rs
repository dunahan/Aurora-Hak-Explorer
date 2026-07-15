//! Native Windows file drag source.

use std::{path::PathBuf, thread, time::Duration};

use tempfile::TempDir;

pub fn release_pointer_grab(_frame: &eframe::Frame) {
    // Windows OLE takes over pointer tracking when start_drag is called.
}

pub fn start(frame: &eframe::Frame, paths: Vec<PathBuf>, temporary_directory: TempDir) {
    let Some(preview) = paths.first().cloned() else {
        return;
    };
    let result = drag::start_drag(
        frame,
        drag::DragItem::Files(paths),
        drag::Image::File(preview),
        |_result, _position| {},
        drag::Options::default(),
    );
    if let Err(error) = result {
        eprintln!("Could not start outgoing file drag: {error}");
    }

    // Explorer can begin its copy job just after the OLE drag loop returns.
    // Keep the exported files alive long enough for that asynchronous copy.
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(300));
        drop(temporary_directory);
    });
}

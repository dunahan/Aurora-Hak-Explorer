//! Native Windows file drag source.

use std::{
    path::PathBuf,
    sync::atomic::{AtomicI64, Ordering},
    thread,
    time::Duration,
};

use tempfile::TempDir;

const NO_POINTER_POSITION: i64 = i64::MIN;
static POINTER_POSITION: AtomicI64 = AtomicI64::new(NO_POINTER_POSITION);

pub fn pointer_position() -> Option<(i32, i32)> {
    let packed = POINTER_POSITION.load(Ordering::Relaxed);
    (packed != NO_POINTER_POSITION).then_some(((packed >> 32) as i32, packed as i32))
}

pub fn release_pointer_grab(_frame: &eframe::Frame) {
    // Windows OLE takes over pointer tracking when start_drag is called.
}

pub fn start(frame: &eframe::Frame, paths: Vec<PathBuf>, temporary_directory: TempDir) {
    let Some(preview) = paths.first().cloned() else {
        return;
    };
    POINTER_POSITION.store(NO_POINTER_POSITION, Ordering::Relaxed);
    let result = drag::start_drag(
        frame,
        drag::DragItem::Files(paths),
        drag::Image::File(preview),
        |_result, position| {
            POINTER_POSITION.store(
                ((position.x as i64) << 32) | position.y as u32 as i64,
                Ordering::Relaxed,
            );
        },
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

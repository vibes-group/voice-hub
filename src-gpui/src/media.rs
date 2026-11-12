use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct CaptureChoice {
    pub id: u64,
    pub title: String,
    pub window: bool,
}
#[derive(Clone)]
pub struct VideoImage {
    pub publisher: String,
    pub width: u32,
    pub height: u32,
    pub bgra: Arc<Vec<u8>>,
}

#[cfg(target_os = "windows")]
#[path = "media_windows.rs"]
mod platform;
#[cfg(target_os = "windows")]
pub use platform::*;

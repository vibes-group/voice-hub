#[cfg(not(all(target_os = "windows", feature = "desktop")))]
use std::path::Path;

pub enum PlayerAction {
    Toggle,
    Seek(f64),
    Volume(f64),
    Rate(f64),
    Resize(i32, i32),
    Stop,
    Fallback,
}
pub enum PlayerEvent {
    Status {
        position: f64,
        duration: f64,
        paused: bool,
    },
    Error(String),
    Frame(crate::media::VideoImage),
}
#[cfg(all(target_os = "windows", feature = "desktop"))]
#[path = "player_windows.rs"]
mod platform;
#[cfg(all(target_os = "windows", feature = "desktop"))]
pub use platform::*;
#[cfg(not(all(target_os = "windows", feature = "desktop")))]
pub struct Player;
#[cfg(not(all(target_os = "windows", feature = "desktop")))]
impl Player {
    pub fn open(
        _: &Path,
        _: &str,
    ) -> Result<(Self, tokio::sync::mpsc::Receiver<PlayerEvent>), String> {
        Err("Нативный видеоплеер доступен в сборке Windows".into())
    }
    pub fn action(&self, _: PlayerAction) {}
    pub fn place(&self, _: i32, _: i32, _: i32, _: i32, _: [i32; 4]) {}
    pub fn hide(&self) {}
}

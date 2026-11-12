#[cfg(not(target_os = "windows"))]
use std::path::Path;

pub enum PlayerAction {
    Toggle,
    Seek(f64),
    Volume(f64),
    Rate(f64),
    Resize(i32, i32),
    Stop,
}
pub enum PlayerEvent {
    Status {
        position: f64,
        duration: f64,
        paused: bool,
    },
    Error(String),
}
#[cfg(target_os = "windows")]
#[path = "player_windows.rs"]
mod platform;
#[cfg(target_os = "windows")]
pub use platform::*;
#[cfg(not(target_os = "windows"))]
pub struct Player;
#[cfg(not(target_os = "windows"))]
impl Player {
    pub fn open(_: &Path) -> Result<(Self, tokio::sync::mpsc::Receiver<PlayerEvent>), String> {
        Err("Нативный видеоплеер доступен в сборке Windows".into())
    }
    pub fn action(&self, _: PlayerAction) {}
    pub fn place(&self, _: i32, _: i32, _: i32, _: i32) {}
    pub fn hide(&self) {}
}

pub mod api;
pub mod audio;
pub mod chat;
#[cfg(feature = "desktop")]
pub mod controller;
#[cfg(feature = "desktop")]
pub mod decoder;
pub mod denoise;
pub mod media;
pub mod player;
pub mod protocol;
pub mod settings;
pub mod system;
pub mod updater;

#[cfg(feature = "desktop")]
pub mod device;
#[cfg(all(feature = "desktop", target_os = "windows"))]
pub mod process_audio;

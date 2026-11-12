use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum NoiseEngine {
    Off,
    WebRtc,
    #[default]
    RnNoise,
    FastTiny,
    FastBase,
}

impl NoiseEngine {
    pub const ALL: [Self; 5] = [
        Self::Off,
        Self::WebRtc,
        Self::RnNoise,
        Self::FastTiny,
        Self::FastBase,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Выкл.",
            Self::WebRtc => "WebRTC",
            Self::RnNoise => "RNNoise",
            Self::FastTiny => "FastEnhancer Tiny",
            Self::FastBase => "FastEnhancer Base",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct PeerSettings {
    pub label: String,
    pub volume: u16,
    pub screen_volume: u16,
    pub muted: bool,
}

impl Default for PeerSettings {
    fn default() -> Self {
        Self {
            label: String::new(),
            volume: 100,
            screen_volume: 100,
            muted: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum Hotkey {
    Keyboard {
        key: String,
        ctrl: bool,
        shift: bool,
        alt: bool,
        meta: bool,
    },
    Mouse(u8),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ScreenSettings {
    pub resolution: String,
    pub fps: u32,
    pub codec: String,
    pub mode: String,
    pub system_audio: bool,
}

impl Default for ScreenSettings {
    fn default() -> Self {
        Self {
            resolution: "source".into(),
            fps: 60,
            codec: "av1".into(),
            mode: "sharp".into(),
            system_audio: true,
        }
    }
}

impl ScreenSettings {
    pub fn dimensions(&self) -> (u32, u32) {
        match self.resolution.as_str() {
            "720" => (1280, 720),
            "1080" => (1920, 1080),
            "1440" => (2560, 1440),
            _ => (3840, 2160),
        }
    }
    pub fn max_bitrate(&self) -> u64 {
        let rates = match self.resolution.as_str() {
            "720" => [1_000_000, 2_000_000, 3_500_000],
            "1080" => [2_000_000, 4_000_000, 6_500_000],
            "1440" => [4_000_000, 8_000_000, 12_000_000],
            _ => [8_000_000, 14_000_000, 20_000_000],
        };
        let bitrate = rates[match self.fps {
            15 => 0,
            30 => 1,
            _ => 2,
        }];
        if self.mode == "sharp" {
            bitrate / 2
        } else {
            bitrate
        }
    }
    pub fn gaming(&mut self) {
        self.resolution = "1080".into();
        self.fps = 60;
        self.mode = "motion".into();
    }
    pub fn presentation(&mut self) {
        self.resolution = "source".into();
        self.fps = 60;
        self.mode = "sharp".into();
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub server: String,
    pub name: String,
    pub client_id: String,
    pub room: String,
    pub voice_active: bool,
    pub muted: bool,
    pub deafened: bool,
    pub pre_deafen_muted: bool,
    pub input_volume: u16,
    pub output_volume: u16,
    pub microphone: String,
    pub speaker: String,
    pub noise: NoiseEngine,
    pub peers: BTreeMap<String, PeerSettings>,
    pub hotkey: Option<Hotkey>,
    pub ping_sound: bool,
    pub mute_pings: bool,
    pub ping_flash: bool,
    pub autostart: bool,
    pub screen: ScreenSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            server: String::new(),
            name: "Гость".into(),
            client_id: uuid::Uuid::new_v4().to_string(),
            room: "room1".into(),
            voice_active: false,
            muted: false,
            deafened: false,
            pre_deafen_muted: false,
            input_volume: 100,
            output_volume: 100,
            microphone: String::new(),
            speaker: String::new(),
            noise: NoiseEngine::default(),
            peers: BTreeMap::new(),
            hotkey: Some(Hotkey::Keyboard {
                key: "KeyM".into(),
                ctrl: true,
                shift: true,
                alt: false,
                meta: false,
            }),
            ping_sound: true,
            mute_pings: false,
            ping_flash: false,
            autostart: false,
            screen: ScreenSettings::default(),
        }
    }
}

impl Settings {
    pub fn load(storage: &Storage) -> Result<Self, String> {
        let mut settings: Self = storage.read("settings.json")?.unwrap_or_default();
        if uuid::Uuid::parse_str(&settings.client_id).is_err() {
            settings.client_id = uuid::Uuid::new_v4().to_string();
        }
        if !matches!(settings.room.as_str(), "room1" | "room2" | "room3") {
            settings.room = "room1".into();
        }
        settings.input_volume = settings.input_volume.min(300);
        settings.output_volume = settings.output_volume.min(300);
        for peer in settings.peers.values_mut() {
            peer.volume = peer.volume.min(500);
            peer.screen_volume = peer.screen_volume.min(500);
            peer.label = peer.label.chars().take(32).collect();
        }
        if settings.deafened {
            settings.muted = true;
        }
        if !matches!(settings.screen.fps, 15 | 30 | 60) {
            settings.screen.fps = 60;
        }
        if !matches!(settings.screen.codec.as_str(), "av1" | "vp9") {
            settings.screen.codec = "av1".into();
        }
        if !matches!(settings.screen.mode.as_str(), "sharp" | "motion") {
            settings.screen.mode = "sharp".into();
        }
        Ok(settings)
    }
    pub fn save(&self, storage: &Storage) -> Result<(), String> {
        storage.write("settings.json", self)
    }
    pub fn toggle_deafen(&mut self) {
        if self.deafened {
            self.deafened = false;
            self.muted = self.pre_deafen_muted;
        } else {
            self.pre_deafen_muted = self.muted;
            self.deafened = true;
            self.muted = true;
        }
    }
    pub fn toggle_mute(&mut self) {
        self.muted = !self.muted;
        self.deafened = false;
    }
    pub fn reset_audio(&mut self) {
        self.input_volume = 100;
        self.output_volume = 100;
        self.noise = NoiseEngine::RnNoise;
        self.microphone.clear();
        self.speaker.clear();
    }
}

#[derive(Clone)]
pub struct Storage {
    root: PathBuf,
}

impl Storage {
    pub fn open() -> Result<Self, String> {
        #[cfg(target_os = "windows")]
        let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
        #[cfg(not(target_os = "windows"))]
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
            });
        Self::at(
            base.ok_or("Не найдена папка данных приложения")?
                .join("VoiceHub-GPUI"),
        )
    }
    pub fn at(root: PathBuf) -> Result<Self, String> {
        std::fs::create_dir_all(&root).map_err(|_| "Не удалось создать папку данных")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
                .map_err(|_| "Не удалось защитить папку данных")?;
        }
        Ok(Self { root })
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn read<T: serde::de::DeserializeOwned>(&self, name: &str) -> Result<Option<T>, String> {
        let path = self.root.join(name);
        match std::fs::read(&path) {
            Ok(data) => serde_json::from_slice(&data)
                .map(Some)
                .map_err(|_| format!("Повреждён файл {name}; исходный файл сохранён")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(format!("Не удалось прочитать {name}")),
        }
    }
    pub fn write(&self, name: &str, value: &impl Serialize) -> Result<(), String> {
        let data = serde_json::to_vec(value).map_err(|_| "Не удалось сериализовать данные")?;
        let temp = self.root.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| {
            use std::io::Write;
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options
                .open(&temp)
                .map_err(|_| "Не удалось сохранить данные")?;
            file.write_all(&data)
                .and_then(|_| file.sync_all())
                .map_err(|_| "Не удалось записать данные")?;
            // Windows rename does not replace an existing file.
            #[cfg(target_os = "windows")]
            if self.root.join(name).exists() {
                std::fs::remove_file(self.root.join(name))
                    .map_err(|_| "Не удалось обновить данные")?;
            }
            std::fs::rename(&temp, self.root.join(name)).map_err(|_| "Не удалось обновить данные")
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(temp);
        }
        result.map_err(str::to_owned)
    }
    pub fn namespace(server: &str, room: &str) -> String {
        use sha2::{Digest, Sha256};
        Sha256::digest(format!("{server}\0{room}"))
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deafen_restores_previous_mute_and_mute_exits_deafen() {
        for muted in [false, true] {
            let mut s = Settings {
                muted,
                ..Settings::default()
            };
            s.toggle_deafen();
            assert!(s.muted && s.deafened);
            s.toggle_deafen();
            assert_eq!(s.muted, muted);
            assert!(!s.deafened);
        }
        let mut s = Settings::default();
        s.toggle_deafen();
        s.toggle_mute();
        assert!(!s.muted && !s.deafened);
    }
    #[test]
    fn settings_round_trip_preserves_identity_and_preferences_without_credentials() {
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        let storage = Storage::at(root.clone()).unwrap();
        let mut s = Settings {
            name: "Tester".into(),
            ..Default::default()
        };
        s.toggle_deafen();
        s.save(&storage).unwrap();
        let loaded = Settings::load(&storage).unwrap();
        assert_eq!(s.client_id, loaded.client_id);
        assert!(loaded.deafened && loaded.muted);
        assert_ne!(
            Storage::namespace("https://one.example", "room1"),
            Storage::namespace("https://two.example", "room1")
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

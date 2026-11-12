use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;

use crate::{
    device::{AudioOutput, StereoPlayback as Playback},
    media::VideoImage,
    player::{PlayerAction, PlayerEvent},
    settings::Storage,
};

#[derive(Deserialize)]
struct Artifact {
    name: String,
    sha256: String,
}
struct Tools {
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
}
impl Drop for Tools {
    fn drop(&mut self) {
        if let Some(folder) = self.ffmpeg.parent() {
            let folder = folder.to_owned();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(1));
                let _ = std::fs::remove_dir_all(folder);
            });
        }
    }
}
#[derive(Deserialize)]
struct Probe {
    streams: Vec<ProbeStream>,
    format: ProbeFormat,
}
#[derive(Deserialize)]
struct ProbeStream {
    codec_type: String,
    width: Option<u32>,
    height: Option<u32>,
    avg_frame_rate: Option<String>,
}
#[derive(Deserialize)]
struct ProbeFormat {
    duration: Option<String>,
}
struct Metadata {
    width: u32,
    height: u32,
    rate: f64,
    duration: f64,
    audio: bool,
}
struct Running {
    tasks: Vec<tokio::task::JoinHandle<()>>,
    clock: Arc<std::sync::OnceLock<Instant>>,
    position: f64,
    rate: f64,
}
impl Running {
    fn position(&self) -> f64 {
        self.position
            + self
                .clock
                .get()
                .map_or(0.0, |at| at.elapsed().as_secs_f64())
                * self.rate
    }
}
impl Drop for Running {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

pub fn run(
    path: PathBuf,
    server: String,
    actions: std::sync::mpsc::Receiver<PlayerAction>,
    events: mpsc::Sender<PlayerEvent>,
) -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| "Не удалось запустить декодер")?;
    runtime.block_on(async move{
        let tools=tools(&server).await?;let metadata=probe(&tools,&path).await?;let output=if metadata.audio{Some(AudioOutput::open("")?)}else{None};
        let(tx,mut rx)=mpsc::channel(8);std::thread::spawn(move||while let Ok(action)=actions.recv(){if tx.blocking_send(action).is_err(){break;}});
        let gain=Arc::new(AtomicU32::new(1.0f32.to_bits()));let mut position=0.0;let mut rate=1.0;let mut running=Some(start(&tools,&path,&metadata,0.0,rate,output.as_ref().map(|o|o.playback.clone()),gain.clone(),events.clone())?);
        let mut tick=tokio::time::interval(Duration::from_millis(100));
        loop{
            tokio::select!{
                action=rx.recv()=>{let Some(action)=action else{break};match action{
                    PlayerAction::Stop=>break,
                    PlayerAction::Volume(volume)=>{gain.store((volume.clamp(0.0,1.0) as f32).to_bits(),Ordering::Relaxed);},
                    PlayerAction::Resize(_,_)|PlayerAction::Fallback=>{},
                    PlayerAction::Toggle=>{if let Some(active)=running.take(){position=active.position();drop(active);if let Some(output)=&output&& let Ok(mut queue)=output.playback.lock(){queue.clear();}}else{if position>=metadata.duration{position=0.0;}running=Some(start(&tools,&path,&metadata,position,rate,output.as_ref().map(|o|o.playback.clone()),gain.clone(),events.clone())?);}},
                    PlayerAction::Seek(next)=>{let playing=running.is_some();running=None;position=next.clamp(0.0,metadata.duration);if let Some(output)=&output&& let Ok(mut queue)=output.playback.lock(){queue.clear();}
                        if playing{running=Some(start(&tools,&path,&metadata,position,rate,output.as_ref().map(|o|o.playback.clone()),gain.clone(),events.clone())?);}},
                    PlayerAction::Rate(next)=>{if let Some(active)=running.take(){position=active.position();rate=next.clamp(0.5,2.0);drop(active);running=Some(start(&tools,&path,&metadata,position,rate,output.as_ref().map(|o|o.playback.clone()),gain.clone(),events.clone())?);}else{rate=next.clamp(0.5,2.0);}},
                }},
                _=tick.tick()=>{if running.as_ref().is_some_and(|active|active.position()>=metadata.duration){running=None;position=metadata.duration;}let _=events.try_send(PlayerEvent::Status{position:running.as_ref().map_or(position,Running::position),duration:metadata.duration,paused:running.is_none()});},
            }
        }Ok(())
    })
}

async fn tools(server: &str) -> Result<Tools, String> {
    let artifact: Artifact = serde_json::from_str(include_str!("../decoder-artifact.json"))
        .map_err(|_| "Некорректный манифест декодера")?;
    let storage = Storage::open()?;
    let folder = storage.root().join("native");
    tokio::fs::create_dir_all(&folder)
        .await
        .map_err(|_| "Не удалось создать папку декодера")?;
    let cached = folder.join(&artifact.name);
    let supplied = std::env::var_os("APP_DECODER_ARCHIVE")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::current_exe().ok().map(|exe| {
                exe.parent()
                    .unwrap_or(Path::new("."))
                    .join("native")
                    .join(&artifact.name)
            })
        });
    let archive = if let Some(path) = supplied.filter(|p| p.is_file()) {
        path
    } else if cached.is_file() {
        cached.clone()
    } else {
        let server = crate::protocol::server_url(server)?;
        let url = server
            .join(&format!("desktop/files/{}", artifact.name))
            .map_err(|_| "Некорректный адрес декодера")?;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(300))
            .build()
            .map_err(|_| "Не удалось настроить загрузку кодеков")?;
        let response = client
            .get(url)
            .send()
            .await
            .map_err(|_| "Не удалось скачать кодеки с вашего сервера")?;
        if !response.status().is_success() {
            return Err("На сервере нет архива кодеков GPUI".into());
        }
        let partial = folder.join(format!("{}.part", uuid::Uuid::new_v4()));
        let result = async {
            use tokio::io::AsyncWriteExt;
            let mut file = tokio::fs::File::create(&partial)
                .await
                .map_err(|_| "Не удалось сохранить кодеки")?;
            let mut stream = response.bytes_stream();
            let mut size = 0;
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| "Загрузка кодеков прервана")?;
                size += chunk.len();
                if size > 128 * 1024 * 1024 {
                    return Err("Архив кодеков слишком большой".to_owned());
                }
                file.write_all(&chunk)
                    .await
                    .map_err(|_| "Не удалось сохранить кодеки")?;
            }
            file.sync_all()
                .await
                .map_err(|_| "Не удалось сохранить кодеки")?;
            drop(file);
            Ok(())
        }
        .await;
        if result.is_err() {
            let _ = tokio::fs::remove_file(&partial).await;
        }
        result?;
        tokio::fs::rename(partial, &cached)
            .await
            .map_err(|_| "Не удалось сохранить кодеки")?;
        cached
    };
    tokio::task::spawn_blocking(move || {
        let mut file =
            std::fs::File::open(&archive).map_err(|_| "Не удалось открыть архив кодеков")?;
        let mut digest = Sha256::new();
        {
            use std::io::Read;
            let mut buffer = [0u8; 65536];
            loop {
                let count = file
                    .read(&mut buffer)
                    .map_err(|_| "Не удалось проверить архив кодеков")?;
                if count == 0 {
                    break;
                }
                digest.update(&buffer[..count]);
            }
        }
        let hash = digest
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        if hash != artifact.sha256 {
            return Err("Контрольная сумма кодеков не совпадает".to_owned());
        }
        use std::io::Seek;
        file.rewind()
            .map_err(|_| "Не удалось прочитать архив кодеков")?;
        let mut zip = zip::ZipArchive::new(file).map_err(|_| "Повреждён архив кодеков")?;
        let destination = folder.join(format!("decoder-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&destination).map_err(|_| "Не удалось распаковать кодеки")?;
        for index in 0..zip.len() {
            let mut entry = zip.by_index(index).map_err(|_| "Повреждён архив кодеков")?;
            let path = entry
                .enclosed_name()
                .ok_or("Небезопасный путь в архиве кодеков")?;
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                != Some("bin")
            {
                continue;
            }
            if !matches!(name, "ffmpeg.exe" | "ffprobe.exe") && !name.ends_with(".dll") {
                continue;
            }
            if entry.size() > 300 * 1024 * 1024 {
                return Err("Компонент кодеков слишком большой".into());
            }
            let mut output = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(destination.join(name))
                .map_err(|_| "Не удалось распаковать кодеки")?;
            std::io::copy(&mut entry, &mut output).map_err(|_| "Не удалось распаковать кодеки")?;
        }
        let tools = Tools {
            ffmpeg: destination.join("ffmpeg.exe"),
            ffprobe: destination.join("ffprobe.exe"),
        };
        if !tools.ffmpeg.is_file() || !tools.ffprobe.is_file() {
            return Err("В архиве отсутствует декодер".into());
        }
        Ok(tools)
    })
    .await
    .map_err(|_| "Не удалось подготовить кодеки")?
}
fn command(executable: &Path) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(executable);
    command
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(target_os = "windows")]
    command.creation_flags(0x08000000);
    command
}
async fn probe(tools: &Tools, path: &Path) -> Result<Metadata, String> {
    let output = tokio::time::timeout(
        Duration::from_secs(15),
        command(&tools.ffprobe)
            .args([
                "-v",
                "error",
                "-protocol_whitelist",
                "file,pipe",
                "-format_whitelist",
                "mov,matroska,webm,avi,ogg,mpeg,mpegts,mp3,wav,flac",
                "-show_entries",
                "stream=codec_type,width,height,avg_frame_rate:format=duration",
                "-of",
                "json",
            ])
            .arg(path)
            .output(),
    )
    .await
    .map_err(|_| "Не удалось прочитать параметры видео")?
    .map_err(|_| "Не удалось запустить декодер")?;
    if !output.status.success() || output.stdout.len() > 1024 * 1024 {
        return Err("Формат видео не поддерживается".into());
    }
    let probe: Probe =
        serde_json::from_slice(&output.stdout).map_err(|_| "Некорректные параметры видео")?;
    let video = probe
        .streams
        .iter()
        .find(|s| s.codec_type == "video")
        .ok_or("В файле нет видео")?;
    let width = video.width.ok_or("Нет ширины видео")?;
    let height = video.height.ok_or("Нет высоты видео")?;
    if width == 0 || height == 0 || width > 16384 || height > 16384 {
        return Err("Неверные размеры видео".into());
    }
    let scale = (3840.0 / width as f64).min(2160.0 / height as f64).min(1.0);
    let width = ((width as f64 * scale) as u32 / 2 * 2).max(2);
    let height = ((height as f64 * scale) as u32 / 2 * 2).max(2);
    let fps = video
        .avg_frame_rate
        .as_deref()
        .and_then(|rate| rate.split_once('/'))
        .and_then(|(a, b)| Some(a.parse::<f64>().ok()? / b.parse::<f64>().ok()?))
        .filter(|fps| fps.is_finite())
        .unwrap_or(30.0)
        .clamp(1.0, 60.0);
    let duration = probe
        .format
        .duration
        .as_deref()
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|s| s.is_finite() && *s > 0.0)
        .ok_or("Не удалось определить длительность видео")?;
    Ok(Metadata {
        width,
        height,
        rate: fps,
        duration,
        audio: probe.streams.iter().any(|s| s.codec_type == "audio"),
    })
}
#[allow(clippy::too_many_arguments)]
fn start(
    tools: &Tools,
    path: &Path,
    metadata: &Metadata,
    position: f64,
    rate: f64,
    playback: Option<Playback>,
    gain: Arc<AtomicU32>,
    events: mpsc::Sender<PlayerEvent>,
) -> Result<Running, String> {
    let mut video = command(&tools.ffmpeg)
        .args([
            "-nostdin",
            "-v",
            "error",
            "-protocol_whitelist",
            "file,pipe",
            "-format_whitelist",
            "mov,matroska,webm,avi,ogg,mpeg,mpegts",
            "-ss",
            &position.to_string(),
            "-i",
        ])
        .arg(path)
        .args([
            "-an",
            "-vf",
            &format!(
                "scale={}:{},fps={}",
                metadata.width, metadata.height, metadata.rate
            ),
            "-pix_fmt",
            "bgra",
            "-f",
            "rawvideo",
            "pipe:1",
        ])
        .spawn()
        .map_err(|_| "Не удалось запустить видео")?;
    let mut stdout = video
        .stdout
        .take()
        .ok_or("Не удалось открыть поток видео")?;
    let (width, height) = (metadata.width, metadata.height);
    let sender = events.clone();
    let clock = Arc::new(std::sync::OnceLock::new());
    let barrier = Arc::new(tokio::sync::Barrier::new(if playback.is_some() {
        2
    } else {
        1
    }));
    let video_clock = clock.clone();
    let video_barrier = barrier.clone();
    let fps = metadata.rate;
    let video = tokio::spawn(async move {
        let _child = video;
        let mut index = 0;
        loop {
            let mut frame = vec![0u8; width as usize * height as usize * 4];
            if stdout.read_exact(&mut frame).await.is_err() {
                if index == 0 {
                    let _ =
                        sender.try_send(PlayerEvent::Error("Не удалось декодировать видео".into()));
                }
                break;
            }
            if index == 0 {
                video_barrier.wait().await;
                video_clock.get_or_init(Instant::now);
            }
            let at = *video_clock.get().expect("video clock");
            tokio::time::sleep_until(tokio::time::Instant::from_std(
                at + Duration::from_secs_f64(index as f64 / fps / rate + 0.04),
            ))
            .await;
            index += 1;
            if sender.is_closed() {
                break;
            }
            let _ = sender.try_send(PlayerEvent::Frame(VideoImage {
                publisher: "attachment".into(),
                width,
                height,
                bgra: Arc::new(frame),
            }));
        }
    });
    let mut tasks = vec![video];
    if let Some(playback) = playback {
        let mut audio = command(&tools.ffmpeg)
            .args([
                "-nostdin",
                "-v",
                "error",
                "-protocol_whitelist",
                "file,pipe",
                "-format_whitelist",
                "mov,matroska,webm,avi,ogg,mpeg,mpegts,mp3,wav,flac",
                "-ss",
                &position.to_string(),
                "-i",
            ])
            .arg(path)
            .args([
                "-vn",
                "-af",
                &format!("atempo={rate}"),
                "-ac",
                "2",
                "-ar",
                "48000",
                "-f",
                "f32le",
                "pipe:1",
            ])
            .spawn()
            .map_err(|_| "Не удалось запустить звук видео")?;
        let mut stdout = audio.stdout.take().ok_or("Не удалось открыть звук видео")?;
        let audio_clock = clock.clone();
        let sender = events.clone();
        tasks.push(tokio::spawn(async move {
            let _child = audio;
            let mut started = false;
            let mut frame = [0u8; 480 * 8];
            while stdout.read_exact(&mut frame).await.is_ok() {
                if !started {
                    barrier.wait().await;
                    audio_clock.get_or_init(Instant::now);
                    started = true;
                }
                loop {
                    let queued = playback.lock().map_or(0, |q| q.len());
                    if queued < 2880 {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                let volume = f32::from_bits(gain.load(Ordering::Relaxed));
                if let Ok(mut queue) = playback.lock() {
                    queue.extend(frame.as_chunks::<8>().0.iter().map(|bytes| {
                        let mut stereo = [0.0; 2];
                        for (channel, to) in stereo.iter_mut().enumerate() {
                            let value = f32::from_le_bytes(
                                bytes[channel * 4..channel * 4 + 4]
                                    .try_into()
                                    .expect("four bytes"),
                            );
                            *to = if value.is_finite() {
                                (value * volume).clamp(-1.0, 1.0)
                            } else {
                                0.0
                            };
                        }
                        stereo
                    }));
                }
            }
            if !started {
                let _ = sender.try_send(PlayerEvent::Error(
                    "Не удалось декодировать звук видео".into(),
                ));
            }
        }));
    }
    Ok(Running {
        tasks,
        clock,
        position,
        rate,
    })
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;
    #[tokio::test]
    async fn bundled_codecs_decode_vp9_and_av1_files_with_audio() {
        let tools = tools("https://unused.example").await.unwrap();
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir(&root).unwrap();
        for codec in ["libvpx-vp9", "libaom-av1"] {
            let path = root.join(format!("{codec}.webm"));
            let result = command(&tools.ffmpeg)
                .args([
                    "-nostdin",
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    "testsrc2=size=160x90:rate=10",
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=440:sample_rate=48000",
                    "-t",
                    "0.6",
                    "-c:v",
                    codec,
                    "-cpu-used",
                    "8",
                    "-threads",
                    "2",
                    "-c:a",
                    "libopus",
                ])
                .arg(&path)
                .status()
                .await
                .unwrap();
            assert!(result.success());
            let metadata = probe(&tools, &path).await.unwrap();
            assert_eq!((metadata.width, metadata.height), (160, 90));
            assert!(metadata.audio);
            assert!(metadata.duration > 0.5);
            let playback = Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new()));
            let (events, mut rx) = mpsc::channel(2);
            let active = start(
                &tools,
                &path,
                &metadata,
                0.0,
                1.0,
                Some(playback.clone()),
                Arc::new(AtomicU32::new(1.0f32.to_bits())),
                events,
            )
            .unwrap();
            let (mut frames, mut audible) = (0, false);
            let mut tick = tokio::time::interval(Duration::from_millis(10));
            tokio::time::timeout(Duration::from_secs(10),async{while frames<2||!audible{tokio::select!{Some(PlayerEvent::Frame(frame))=rx.recv()=>{assert_eq!(frame.bgra.len(),160*90*4);frames+=1;},_=tick.tick()=>{let mut queue=playback.lock().unwrap();audible|=queue.iter().flatten().any(|sample|sample.abs()>0.01);queue.clear();}}}}).await.unwrap();
            drop(active);
            tokio::time::sleep(Duration::from_millis(100)).await;
            let (events, mut rx) = mpsc::channel(2);
            let seek = start(
                &tools,
                &path,
                &metadata,
                0.2,
                1.0,
                None,
                Arc::new(AtomicU32::new(1.0f32.to_bits())),
                events,
            )
            .unwrap();
            assert!(matches!(
                tokio::time::timeout(Duration::from_secs(5), rx.recv())
                    .await
                    .unwrap(),
                Some(PlayerEvent::Frame(_))
            ));
            drop(seek);
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}

//! Synthetic load against a running voice-hub server (any implementation of
//! the protocol). Scenarios model production traffic; results go to JSON and
//! `report` compares runs.
//!
//!     printf %s "$PASS" | loadtest run --target http://127.0.0.1:8080 \
//!         --password-stdin --scenario office --duration 60 --server-pid 1234 \
//!         --label rust --out rust-office.json
//!     loadtest report go-office.json rust-office.json
//!
//! Scenarios:
//!   office   3 rooms (6/4/2 voice + lurkers), rotating speaker, DTX, mute,
//!            chat, state toggles, presence watchers
//!   screen   8 voice peers, one VP9 L1T3 2.5 Mbps screen share + system
//!            audio watched by everyone (one viewer capped to T1)
//!   churn    16 steady voice peers + 4 peers reconnecting (same clientId)
//!   scale:N  N voice peers in one room, all speaking (worst-case fan-out)
//!   smoke    4 peers speaking; exits non-zero when nothing flows

use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use voice_hub::loadclient::{self, ClientPeer, PeerOpts, Stats, Target};

struct Args {
    target: String,
    password: String,
    scenario: String,
    duration: Duration,
    server_pid: Option<u32>,
    label: String,
    out: Option<String>,
    local_ip: std::net::IpAddr,
}

fn usage() -> ! {
    eprintln!(
        "usage: loadtest run --target URL (--password P | --password-stdin) --scenario NAME [--duration SECS] [--server-pid PID] [--label L] [--out FILE]\n       loadtest report A.json B.json [...]"
    );
    std::process::exit(2);
}

fn parse_args(mut it: impl Iterator<Item = String>) -> Args {
    let mut a = Args {
        target: "http://127.0.0.1:8080".into(),
        password: String::new(),
        scenario: "smoke".into(),
        duration: Duration::from_secs(30),
        server_pid: None,
        label: "server".into(),
        out: None,
        local_ip: "127.0.0.1".parse().unwrap(),
    };
    while let Some(flag) = it.next() {
        let mut val = || it.next().unwrap_or_else(|| usage());
        match flag.as_str() {
            "--target" => a.target = val(),
            "--password" => a.password = val(),
            "--password-stdin" => {
                let mut s = String::new();
                std::io::stdin().read_to_string(&mut s).expect("read stdin");
                a.password = s.trim_end_matches(['\r', '\n']).to_string();
            }
            "--scenario" => a.scenario = val(),
            "--duration" => a.duration = Duration::from_secs(val().parse().unwrap_or_else(|_| usage())),
            "--server-pid" => a.server_pid = Some(val().parse().unwrap_or_else(|_| usage())),
            "--label" => a.label = val(),
            "--out" => a.out = Some(val()),
            "--local-ip" => a.local_ip = val().parse().unwrap_or_else(|_| usage()),
            _ => usage(),
        }
    }
    if a.password.is_empty() {
        usage();
    }
    a
}

// --- Server resource sampling (/proc) ---

#[derive(Default, Clone, Serialize, Deserialize)]
struct ServerSample {
    t: f64,
    cpu_pct: f64,
    rss_mb: f64,
    threads: u64,
}

fn proc_read(pid: u32) -> Option<(u64, u64, u64)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = &stat[stat.rfind(')')? + 2..];
    let f: Vec<&str> = rest.split_whitespace().collect();
    let ticks = f.get(11)?.parse::<u64>().ok()? + f.get(12)?.parse::<u64>().ok()?;
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let field = |name: &str| {
        status.lines().find(|l| l.starts_with(name)).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse::<u64>().ok())
    };
    Some((ticks, field("VmRSS:")?, field("Threads:")?))
}

async fn sample_server(pid: u32, start: Instant, cancel: CancellationToken) -> Vec<ServerSample> {
    // Linux USER_HZ is 100 on every platform we run on.
    const HZ: f64 = 100.0;
    let mut out = Vec::new();
    let mut prev = proc_read(pid);
    let mut prev_t = Instant::now();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.tick().await;
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return out,
            _ = tick.tick() => {}
        }
        let Some(cur) = proc_read(pid) else { return out };
        let dt = prev_t.elapsed().as_secs_f64();
        if let Some(p) = prev {
            out.push(ServerSample {
                t: start.elapsed().as_secs_f64(),
                cpu_pct: (cur.0 - p.0) as f64 / HZ / dt * 100.0,
                rss_mb: cur.1 as f64 / 1024.0,
                threads: cur.2,
            });
        }
        prev = Some(cur);
        prev_t = Instant::now();
    }
}

// --- Results ---

#[derive(Default, Serialize, Deserialize, Clone)]
struct Dist {
    count: u64,
    p50: f64,
    p95: f64,
    p99: f64,
    max: f64,
}

fn dist_ms(mut v: Vec<u32>) -> Dist {
    if v.is_empty() {
        return Dist::default();
    }
    v.sort_unstable();
    let q = |p: f64| v[((v.len() - 1) as f64 * p).round() as usize] as f64 / 1000.0;
    Dist { count: v.len() as u64, p50: q(0.5), p95: q(0.95), p99: q(0.99), max: *v.last().unwrap() as f64 / 1000.0 }
}

#[derive(Default, Serialize, Deserialize, Clone)]
struct Media {
    packets: u64,
    lost: u64,
    loss_pct: f64,
    pps: f64,
    latency_ms: Dist,
}

#[derive(Default, Serialize, Deserialize, Clone)]
struct ServerStats {
    cpu_avg_pct: f64,
    cpu_p95_pct: f64,
    rss_avg_mb: f64,
    rss_max_mb: f64,
    threads_max: u64,
    samples: Vec<ServerSample>,
}

#[derive(Default, Serialize, Deserialize, Clone)]
struct Report {
    label: String,
    scenario: String,
    duration_s: f64,
    peers: u64,
    connected: u64,
    connect_errors: u64,
    disconnects: u64,
    signaling_errors: u64,
    ws_messages_in: u64,
    presence_events: u64,
    audio: Media,
    video: Media,
    video_frames: u64,
    chat_latency_ms: Dist,
    welcome_ms: Dist,
    first_media_ms: Dist,
    server: ServerStats,
}

fn media(accs: Vec<loadclient::StreamAcc>, lat: Vec<u32>, secs: f64) -> Media {
    let packets: u64 = accs.iter().map(|a| a.count).sum();
    let lost: u64 = accs.iter().map(|a| a.lost()).sum();
    Media {
        packets,
        lost,
        loss_pct: if packets + lost == 0 { 0.0 } else { lost as f64 * 100.0 / (packets + lost) as f64 },
        pps: packets as f64 / secs,
        latency_ms: dist_ms(lat),
    }
}

// --- Scenario plumbing ---

struct Run {
    target: Arc<Target>,
    peers: tokio::sync::Mutex<Vec<Arc<ClientPeer>>>,
    finished: std::sync::Mutex<Vec<Arc<Stats>>>,
    connect_errors: AtomicU64,
    presence_events: Arc<AtomicU64>,
    cancel: CancellationToken,
    started: Instant,
}

impl Run {
    async fn join(
        &self,
        idx: u32,
        room: &str,
        chat_only: bool,
        watch: bool,
        layer: u8,
        client_id: Option<String>,
    ) -> Option<Arc<ClientPeer>> {
        let opts = PeerOpts {
            display_name: format!("p{idx}"),
            client_id: client_id.unwrap_or_else(|| format!("lt-{idx}")),
            chat_only,
            room: room.into(),
            watch_screens: watch,
            temporal_layer: layer,
        };
        match ClientPeer::connect(self.target.clone(), idx, opts, Arc::new(Stats::default())).await {
            Ok(p) => {
                self.peers.lock().await.push(p.clone());
                Some(p)
            }
            Err(e) => {
                eprintln!("peer {idx}: {e}");
                self.connect_errors.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }

    /// Closes a peer but keeps its measurements.
    async fn leave(&self, p: &Arc<ClientPeer>) {
        p.close();
        let mut peers = self.peers.lock().await;
        peers.retain(|q| !Arc::ptr_eq(q, p));
        self.finished.lock().unwrap().push(p.stats.clone());
    }

    fn watch_presence(&self, n: usize) {
        for _ in 0..n {
            let (t, ev, c) = (self.target.clone(), self.presence_events.clone(), self.cancel.clone());
            tokio::spawn(async move {
                let _ = loadclient::presence_watch(t, ev, c).await;
            });
        }
    }
}

/// Drives 20 ms audio ticks: `speaking(peer, tick)` → Some(true) full frame,
/// Some(false) DTX (one packet per 400 ms), None muted.
fn audio_driver(run: Arc<Run>, speaking: impl Fn(&ClientPeer, u64) -> Option<bool> + Send + 'static) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(20));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Burst);
        let mut n: u64 = 0;
        loop {
            tokio::select! {
                _ = run.cancel.cancelled() => return,
                _ = tick.tick() => {}
            }
            n += 1;
            let peers = run.peers.lock().await.clone();
            for p in &peers {
                match speaking(p, n) {
                    Some(true) => p.send_audio(true, (n * 960) as u32),
                    Some(false) if n.is_multiple_of(20) => p.send_audio(false, (n * 960) as u32),
                    _ => {}
                }
            }
        }
    });
}

async fn office(run: Arc<Run>) {
    let rooms = [("room1", 6, 2), ("room2", 4, 1), ("room3", 2, 0)];
    run.watch_presence(4);
    let mut idx = 0;
    for (room, voice, lurkers) in rooms {
        for _ in 0..voice {
            run.join(idx, room, false, false, 2, None).await;
            idx += 1;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        for _ in 0..lurkers {
            run.join(idx, room, true, false, 2, None).await;
            idx += 1;
        }
    }
    // Room membership by index: rotating speaker per room every 4 s; the
    // last voice peer in each room stays muted.
    let layout: Vec<(u32, u32)> = vec![(0, 6), (8, 4), (13, 2)];
    audio_driver(run.clone(), move |p, n| {
        let (base, count) = *layout.iter().find(|(b, c)| p.idx >= *b && p.idx < b + c)?;
        let local = p.idx - base;
        if local == count - 1 && count > 2 {
            return None;
        }
        let speaker = ((n / 200) % (count as u64 - if count > 2 { 1 } else { 0 })) as u32;
        Some(local == speaker)
    });
    let r = run.clone();
    tokio::spawn(async move {
        let mut i: u64 = 0;
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        loop {
            tokio::select! {
                _ = r.cancel.cancelled() => return,
                _ = tick.tick() => {}
            }
            i += 1;
            let peers = r.peers.lock().await.clone();
            if peers.is_empty() {
                continue;
            }
            if i.is_multiple_of(3) {
                peers[(i as usize * 7) % peers.len()].chat("synthetic chat line, about forty bytes");
            }
            if i.is_multiple_of(7) {
                peers[(i as usize * 5) % peers.len()].set_state(i.is_multiple_of(14));
            }
        }
    });
}

async fn screen(run: Arc<Run>) {
    run.watch_presence(2);
    for i in 0..8 {
        run.join(i, "room1", false, i != 0, if i == 7 { 1 } else { 2 }, None).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    audio_driver(run.clone(), |p, n| Some(p.idx == ((n / 200) % 8) as u32));
    tokio::time::sleep(Duration::from_secs(2)).await;
    let Some(publisher) = run.peers.lock().await.first().cloned() else { return };
    if publisher.start_screen(true).await.is_err() {
        return;
    }
    let r = run.clone();
    tokio::spawn(async move {
        // 30 fps, ~2.5 Mbit/s average.
        let mut tick = tokio::time::interval(Duration::from_micros(33_333));
        let (mut frame, mut seq, mut aseq) = (0u32, 5000u64, 7000u64);
        let mut audio_tick = 0u32;
        loop {
            tokio::select! {
                _ = r.cancel.cancelled() => return,
                _ = tick.tick() => {}
            }
            if publisher.send_screen_frame(frame, 10_400, &mut seq) {
                frame += 1;
            }
            // System audio at 50 pps rides along (≈ 1.5 packets per frame).
            let due = (frame as u64 * 50 / 30) as u32;
            while audio_tick < due {
                publisher.send_screen_audio(aseq, audio_tick * 960);
                aseq += 1;
                audio_tick += 1;
            }
        }
    });
}

async fn churn(run: Arc<Run>) {
    for i in 0..16 {
        run.join(i, "room1", false, false, 2, None).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    audio_driver(run.clone(), |p, n| Some(p.idx == ((n / 150) % 16) as u32 || p.idx == ((n / 150 + 8) % 16) as u32));
    let r = run.clone();
    tokio::spawn(async move {
        let mut gen_ = 0u32;
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        loop {
            tokio::select! {
                _ = r.cancel.cancelled() => return,
                _ = tick.tick() => {}
            }
            let slot = gen_ % 4;
            let old = r.peers.lock().await.iter().find(|p| p.idx >= 100 && (p.idx - 100) % 4 == slot).cloned();
            // Every other reconnect overlaps the old session (eviction path).
            if let Some(old) = &old
                && gen_.is_multiple_of(2)
            {
                r.leave(old).await;
            }
            r.join(100 + gen_ * 4 + slot, "room1", false, false, 2, Some(format!("churn-{slot}"))).await;
            if let Some(old) = &old
                && gen_ % 2 == 1
            {
                r.leave(old).await;
            }
            gen_ += 1;
        }
    });
}

async fn scale(run: Arc<Run>, n: u32) {
    for i in 0..n {
        run.join(i, "room1", false, false, 2, None).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    audio_driver(run.clone(), |_, _| Some(true));
}

#[tokio::main]
async fn main() {
    let mut argv = std::env::args().skip(1);
    match argv.next().as_deref() {
        Some("run") => run_cmd(parse_args(argv)).await,
        Some("report") => report_cmd(argv.collect()),
        _ => usage(),
    }
}

async fn run_cmd(a: Args) {
    let url = a.target.trim_start_matches("http://");
    let (host, port) =
        url.split_once(':').map(|(h, p)| (h.to_string(), p.trim_end_matches('/').parse().unwrap_or(80))).unwrap_or((url.to_string(), 80));
    let cookie = loadclient::login(&host, port, &a.password).await.unwrap_or_else(|e| {
        eprintln!("login: {e}");
        std::process::exit(1);
    });
    let run = Arc::new(Run {
        target: Arc::new(Target { host, port, cookie, local_ip: a.local_ip }),
        peers: tokio::sync::Mutex::new(Vec::new()),
        finished: std::sync::Mutex::new(Vec::new()),
        connect_errors: AtomicU64::new(0),
        presence_events: Arc::new(AtomicU64::new(0)),
        cancel: CancellationToken::new(),
        started: Instant::now(),
    });
    let sampler = a.server_pid.map(|pid| tokio::spawn(sample_server(pid, run.started, run.cancel.clone())));

    let (scenario, peers_planned) = match a.scenario.as_str() {
        "office" => {
            office(run.clone()).await;
            ("office", 15)
        }
        "screen" => {
            screen(run.clone()).await;
            ("screen", 8)
        }
        "churn" => {
            churn(run.clone()).await;
            ("churn", 20)
        }
        "smoke" => {
            scale(run.clone(), 4).await;
            ("smoke", 4)
        }
        s if s.starts_with("scale:") => {
            let n: u32 = s[6..].parse().unwrap_or_else(|_| usage());
            scale(run.clone(), n).await;
            ("scale", n as u64)
        }
        _ => usage(),
    };
    let measure_from = Instant::now();
    let mut ticker = tokio::time::interval(Duration::from_secs(5));
    ticker.tick().await;
    while measure_from.elapsed() < a.duration {
        ticker.tick().await;
        let peers = run.peers.lock().await;
        let pkts: u64 = peers.iter().map(|p| p.stats.audio.lock().unwrap().values().map(|s| s.count).sum::<u64>()).sum();
        eprintln!("t={:.0}s peers={} audio_rx={pkts}", run.started.elapsed().as_secs_f64(), peers.len());
    }
    run.cancel.cancel();
    let samples = match sampler {
        Some(h) => h.await.unwrap_or_default(),
        None => Vec::new(),
    };

    let peers = run.peers.lock().await.clone();
    let connected = peers.iter().filter(|p| p.joined()).count() as u64;
    for p in &peers {
        p.close();
    }
    let mut all: Vec<Arc<Stats>> = peers.iter().map(|p| p.stats.clone()).collect();
    all.extend(run.finished.lock().unwrap().iter().cloned());
    let secs = run.started.elapsed().as_secs_f64();

    let mut audio_acc = Vec::new();
    let mut video_acc = Vec::new();
    let (mut a_lat, mut v_lat, mut chat, mut welcome, mut first) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let (mut frames, mut ws, mut disc, mut errs) = (0, 0, 0, 0);
    for s in &all {
        audio_acc.extend(s.audio.lock().unwrap().values().copied());
        video_acc.extend(s.video.lock().unwrap().values().copied());
        let mut sm = s.samples.lock().unwrap();
        a_lat.append(&mut sm.audio_us);
        v_lat.append(&mut sm.video_us);
        chat.append(&mut sm.chat_us);
        welcome.append(&mut sm.welcome_us);
        first.append(&mut sm.first_media_us);
        frames += s.video_frames.load(Ordering::Relaxed);
        ws += s.ws_in.load(Ordering::Relaxed);
        disc += s.disconnects.load(Ordering::Relaxed);
        errs += s.errors.load(Ordering::Relaxed);
    }
    // Skip the first 5 s (join/ramp) for steady-state resource numbers.
    let steady: Vec<&ServerSample> =
        samples.iter().filter(|s| s.t > measure_from.duration_since(run.started).as_secs_f64() + 5.0).collect();
    let mut cpus: Vec<f64> = steady.iter().map(|s| s.cpu_pct).collect();
    cpus.sort_by(|x, y| x.partial_cmp(y).unwrap());
    let server = ServerStats {
        cpu_avg_pct: if cpus.is_empty() { 0.0 } else { cpus.iter().sum::<f64>() / cpus.len() as f64 },
        cpu_p95_pct: cpus.get(((cpus.len().max(1) - 1) as f64 * 0.95) as usize).copied().unwrap_or(0.0),
        rss_avg_mb: if steady.is_empty() { 0.0 } else { steady.iter().map(|s| s.rss_mb).sum::<f64>() / steady.len() as f64 },
        rss_max_mb: samples.iter().map(|s| s.rss_mb).fold(0.0, f64::max),
        threads_max: samples.iter().map(|s| s.threads).max().unwrap_or(0),
        samples,
    };
    let report = Report {
        label: a.label,
        scenario: if scenario == "scale" { a.scenario.clone() } else { scenario.into() },
        duration_s: secs,
        peers: peers_planned,
        connected,
        connect_errors: run.connect_errors.load(Ordering::Relaxed),
        disconnects: disc,
        signaling_errors: errs,
        ws_messages_in: ws,
        presence_events: run.presence_events.load(Ordering::Relaxed),
        audio: media(audio_acc, a_lat, secs),
        video: media(video_acc, v_lat, secs),
        video_frames: frames,
        chat_latency_ms: dist_ms(chat),
        welcome_ms: dist_ms(welcome),
        first_media_ms: dist_ms(first),
        server,
    };
    let json = serde_json::to_string_pretty(&report).unwrap();
    match &a.out {
        Some(path) => std::fs::write(path, &json).expect("write report"),
        None => println!("{json}"),
    }
    eprintln!(
        "{} {}: audio {} pkts loss {:.3}% p50 {:.2}ms p99 {:.2}ms | video {} pkts loss {:.3}% | cpu {:.1}% rss {:.1}MB",
        report.label,
        report.scenario,
        report.audio.packets,
        report.audio.loss_pct,
        report.audio.latency_ms.p50,
        report.audio.latency_ms.p99,
        report.video.packets,
        report.video.loss_pct,
        report.server.cpu_avg_pct,
        report.server.rss_max_mb
    );
    if scenario == "smoke" && (connected < peers_planned || report.audio.packets == 0) {
        eprintln!("FAIL: connected {connected}/{peers_planned}, audio packets {}", report.audio.packets);
        std::process::exit(1);
    }
    std::process::exit(0);
}

fn report_cmd(files: Vec<String>) {
    if files.is_empty() {
        usage();
    }
    let reports: Vec<Report> =
        files.iter().map(|f| serde_json::from_str(&std::fs::read_to_string(f).expect("read report")).expect("parse report")).collect();
    let head: Vec<String> = reports.iter().map(|r| r.label.clone()).collect();
    println!("### {}\n", reports[0].scenario);
    println!("| metric | {} |", head.join(" | "));
    println!("|---|{}|", vec!["---:"; head.len()].join("|"));
    let row = |name: &str, f: &dyn Fn(&Report) -> String| {
        println!("| {name} | {} |", reports.iter().map(f).collect::<Vec<_>>().join(" | "));
    };
    row("peers connected", &|r| format!("{}/{}", r.connected, r.peers));
    row("connect errors / disconnects", &|r| format!("{} / {}", r.connect_errors, r.disconnects));
    row("server CPU avg, %", &|r| format!("{:.1}", r.server.cpu_avg_pct));
    row("server CPU p95, %", &|r| format!("{:.1}", r.server.cpu_p95_pct));
    row("server RSS avg / max, MB", &|r| format!("{:.1} / {:.1}", r.server.rss_avg_mb, r.server.rss_max_mb));
    row("server threads max", &|r| r.server.threads_max.to_string());
    row("audio delivered, pps", &|r| format!("{:.0}", r.audio.pps));
    row("audio loss, %", &|r| format!("{:.3}", r.audio.loss_pct));
    row("audio latency p50 / p95 / p99, ms", &|r| {
        format!("{:.2} / {:.2} / {:.2}", r.audio.latency_ms.p50, r.audio.latency_ms.p95, r.audio.latency_ms.p99)
    });
    if reports.iter().any(|r| r.video.packets > 0) {
        row("video delivered, pps", &|r| format!("{:.0}", r.video.pps));
        row("video loss, %", &|r| format!("{:.3}", r.video.loss_pct));
        row("video latency p50 / p95 / p99, ms", &|r| {
            format!("{:.2} / {:.2} / {:.2}", r.video.latency_ms.p50, r.video.latency_ms.p95, r.video.latency_ms.p99)
        });
        row("video frames received", &|r| r.video_frames.to_string());
    }
    if reports.iter().any(|r| r.chat_latency_ms.count > 0) {
        row("chat latency p50 / p95, ms", &|r| format!("{:.2} / {:.2}", r.chat_latency_ms.p50, r.chat_latency_ms.p95));
    }
    row("hello→welcome p50 / p95, ms", &|r| format!("{:.2} / {:.2}", r.welcome_ms.p50, r.welcome_ms.p95));
    row("connect→first media p50 / p95, ms", &|r| format!("{:.0} / {:.0}", r.first_media_ms.p50, r.first_media_ms.p95));
    if reports.iter().any(|r| r.presence_events > 0) {
        row("presence events", &|r| r.presence_events.to_string());
    }
    println!();
}

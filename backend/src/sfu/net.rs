//! UDP transport and the driver around str0m's sans-IO `Rtc`.
//!
//! All PeerConnections share a few UDP sockets from the configured port range.
//! Inbound datagrams are demultiplexed by the STUN USERNAME (local ufrag) and,
//! once a path is known, by source address. Each `Pc` is processed inline on
//! the socket task that received its datagram: no per-packet channel hop.
//!
//! Locking: `Pc::rtc` guards the `Rtc`; `Pc::dispatch` serialises event
//! delivery so a PC's events reach its handler in order. A handler may write
//! into *other* PCs (`Pc::write`), which takes only their `rtc` lock and
//! never their `dispatch` lock, so two PCs forwarding to each other cannot
//! deadlock.

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::time::{Duration, Instant};

use str0m::net::{Protocol, Receive};
use str0m::{Event, IceConnectionState, Input, Output, Rtc};
use tokio::net::UdpSocket;
use tokio::sync::Notify;

use crate::log;

/// Lock that survives a panic in another holder (state stays usable; the
/// panicking PC is closed by its caller).
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A PC that is not ICE-connected for this long is torn down (pion's
/// disconnected + failed timeouts).
const ICE_FAIL_AFTER: Duration = Duration::from_secs(30);
const MAX_SOCKETS: usize = 16;

pub trait PcHandler: Send + Sync {
    fn on_event(&self, pc: &Arc<Pc>, ev: Event);
    /// Called once when the PC dies (ICE failure, DTLS close, explicit close).
    fn on_closed(&self, pc: &Arc<Pc>);
}

pub struct Mux {
    sockets: Vec<Arc<MuxSocket>>,
    next: AtomicUsize,
    stop: tokio_util::sync::CancellationToken,
}

pub struct MuxSocket {
    socket: UdpSocket,
    /// Address advertised in the host candidate (public IP, bound port).
    pub public_addr: SocketAddr,
    routes: RwLock<HashMap<SocketAddr, Weak<Pc>>>,
    ufrags: RwLock<HashMap<String, Weak<Pc>>>,
}

impl Mux {
    /// Binds up to `MAX_SOCKETS` ports from `[min, max]` (one per core) and
    /// starts their receive loops.
    pub async fn bind(public_ip: std::net::IpAddr, min: u16, max: u16) -> std::io::Result<Arc<Mux>> {
        let want = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, MAX_SOCKETS);
        let mut sockets = Vec::new();
        // min = 0: one OS-assigned port (tests, load clients).
        let ports = if min == 0 { 0..=0 } else { min..=max };
        for port in ports {
            if sockets.len() == want {
                break;
            }
            match UdpSocket::bind(("0.0.0.0", port)).await {
                Ok(socket) => {
                    let public_addr = SocketAddr::new(public_ip, socket.local_addr()?.port());
                    sockets.push(Arc::new(MuxSocket { socket, public_addr, routes: RwLock::default(), ufrags: RwLock::default() }));
                }
                Err(e) if min == 0 => return Err(e),
                Err(e) => log!("sfu: udp bind :{port}: {e}"),
            }
        }
        if sockets.is_empty() {
            return Err(std::io::Error::other(format!("no free UDP port in {min}-{max}")));
        }
        let stop = tokio_util::sync::CancellationToken::new();
        for s in &sockets {
            tokio::spawn(s.clone().recv_loop(stop.clone()));
        }
        Ok(Arc::new(Mux { sockets, next: AtomicUsize::new(0), stop }))
    }

    /// Stops the receive loops so the sockets close once their PCs are gone.
    pub fn close(&self) {
        self.stop.cancel();
    }

    pub fn pick(&self) -> Arc<MuxSocket> {
        let i = self.next.fetch_add(1, Ordering::Relaxed) % self.sockets.len();
        self.sockets[i].clone()
    }
}

impl MuxSocket {
    async fn recv_loop(self: Arc<Self>, stop: tokio_util::sync::CancellationToken) {
        let mut buf = vec![0u8; 2048];
        loop {
            let r = tokio::select! {
                _ = stop.cancelled() => return,
                r = self.socket.recv_from(&mut buf) => r,
            };
            let (n, src) = match r {
                Ok(v) => v,
                Err(e) => {
                    // ICMP port-unreachable and friends surface here; keep going.
                    if e.kind() != std::io::ErrorKind::ConnectionRefused {
                        log!("sfu: udp recv: {e}");
                    }
                    continue;
                }
            };
            let data = &buf[..n];
            let by_ufrag = stun_local_ufrag(data).and_then(|ufrag| self.ufrags.read().unwrap().get(ufrag).and_then(Weak::upgrade));
            let pc = match by_ufrag {
                Some(pc) => {
                    self.learn(src, &pc);
                    Some(pc)
                }
                None => self.routes.read().unwrap().get(&src).and_then(Weak::upgrade),
            };
            if let Some(pc) = pc {
                // One misbehaving PC must not take down every PC on this socket.
                let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pc.receive(src, self.public_addr, data)));
                if r.is_err() {
                    log!("sfu: pc {} panicked; closing it", pc.id);
                    pc.close();
                }
            }
        }
    }

    fn learn(&self, src: SocketAddr, pc: &Arc<Pc>) {
        let known = self.routes.read().unwrap().get(&src).is_some_and(|w| w.as_ptr() == Arc::as_ptr(pc));
        if !known {
            self.routes.write().unwrap().insert(src, Arc::downgrade(pc));
            lock(&pc.rtc).remotes.push(src);
        }
    }

    fn send(&self, dst: SocketAddr, data: &[u8]) {
        // UDP: a full send buffer means drop, like a congested link.
        let _ = self.socket.try_send_to(data, dst);
    }
}

/// For a STUN binding request, the local ufrag from USERNAME ("local:remote").
fn stun_local_ufrag(b: &[u8]) -> Option<&str> {
    if b.len() < 20 || b[0] & 0xC0 != 0 || b[4..8] != [0x21, 0x12, 0xA4, 0x42] {
        return None;
    }
    let len = u16::from_be_bytes([b[2], b[3]]) as usize;
    let attrs = b.get(20..20 + len)?;
    let mut i = 0;
    while i + 4 <= attrs.len() {
        let typ = u16::from_be_bytes([attrs[i], attrs[i + 1]]);
        let alen = u16::from_be_bytes([attrs[i + 2], attrs[i + 3]]) as usize;
        let val = attrs.get(i + 4..i + 4 + alen)?;
        if typ == 0x0006 {
            let user = std::str::from_utf8(val).ok()?;
            return Some(user.split(':').next().unwrap_or(user));
        }
        i += 4 + alen.div_ceil(4) * 4;
    }
    None
}

pub struct Pc {
    pub id: u64,
    rtc: Mutex<RtcState>,
    dispatch: Mutex<()>,
    socket: Arc<MuxSocket>,
    handler: Weak<dyn PcHandler>,
    timer: Notify,
    /// Nanoseconds since `epoch` of the next str0m timeout.
    deadline: AtomicU64,
    epoch: Instant,
    closed: AtomicBool,
    /// Changes on an ICE restart.
    ufrag: Mutex<String>,
    me: Weak<Pc>,
}

struct RtcState {
    rtc: Rtc,
    events: VecDeque<Event>,
    remotes: Vec<SocketAddr>,
    /// Since when ICE has not been connected; None while connected.
    unhealthy_since: Option<Instant>,
}

static NEXT_PC_ID: AtomicU64 = AtomicU64::new(1);

impl Pc {
    /// Registers `rtc` on a socket and starts its timer task. The caller has
    /// already added the socket's host candidate (see `rtc::build`).
    pub fn spawn(rtc: Rtc, socket: Arc<MuxSocket>, ufrag: String, handler: Weak<dyn PcHandler>) -> Arc<Pc> {
        let now = Instant::now();
        let pc = Arc::new_cyclic(|me| Pc {
            id: NEXT_PC_ID.fetch_add(1, Ordering::Relaxed),
            rtc: Mutex::new(RtcState { rtc, events: VecDeque::new(), remotes: Vec::new(), unhealthy_since: Some(now) }),
            dispatch: Mutex::new(()),
            socket: socket.clone(),
            handler,
            timer: Notify::new(),
            deadline: AtomicU64::new(0),
            epoch: now,
            closed: AtomicBool::new(false),
            ufrag: Mutex::new(ufrag.clone()),
            me: me.clone(),
        });
        socket.ufrags.write().unwrap().insert(ufrag, Arc::downgrade(&pc));
        tokio::spawn(pc.clone().timer_loop());
        pc
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// Runs `f` on the Rtc, flushes output, then delivers events in order.
    pub fn with<R>(self: &Arc<Self>, f: impl FnOnce(&mut Rtc) -> R) -> R {
        let _order = lock(&self.dispatch);
        let r = {
            let mut st = lock(&self.rtc);
            let r = f(&mut st.rtc);
            self.flush(&mut st);
            r
        };
        self.deliver();
        r
    }

    /// Writes into this PC from another PC's event handler: takes only the
    /// Rtc lock. Events produced here are delivered on this PC's next drive.
    pub fn write(self: &Arc<Self>, f: impl FnOnce(&mut Rtc)) {
        if self.is_closed() {
            return;
        }
        let mut st = lock(&self.rtc);
        f(&mut st.rtc);
        self.flush(&mut st);
        if !st.events.is_empty() {
            self.timer.notify_one();
        }
    }

    fn receive(self: &Arc<Self>, src: SocketAddr, dst: SocketAddr, data: &[u8]) {
        let Ok(contents) = data.try_into() else { return };
        self.with(|rtc| {
            let input = Input::Receive(Instant::now(), Receive { proto: Protocol::Udp, source: src, destination: dst, contents });
            if let Err(e) = rtc.handle_input(input) {
                log!("sfu: pc {} input from {src}: {e}", self.id);
                rtc.disconnect();
            }
        });
    }

    /// Polls str0m until it asks for a timeout; sends datagrams and queues events.
    fn flush(&self, st: &mut RtcState) {
        loop {
            if !st.rtc.is_alive() {
                return;
            }
            match st.rtc.poll_output() {
                Ok(Output::Transmit(t)) => {
                    if !st.remotes.contains(&t.destination) {
                        st.remotes.push(t.destination);
                        // A response to our own connectivity check carries no
                        // USERNAME; route it by address.
                        self.socket.routes.write().unwrap().insert(t.destination, self.me.clone());
                    }
                    self.socket.send(t.destination, &t.contents);
                }
                Ok(Output::Timeout(t)) => {
                    let ns = t.saturating_duration_since(self.epoch).as_nanos() as u64;
                    let prev = self.deadline.swap(ns, Ordering::AcqRel);
                    if ns < prev || prev == 0 {
                        self.timer.notify_one();
                    }
                    return;
                }
                Ok(Output::Event(ev)) => {
                    if let Event::IceConnectionStateChange(s) = &ev {
                        st.unhealthy_since = match s {
                            IceConnectionState::Connected | IceConnectionState::Completed => None,
                            _ => st.unhealthy_since.or(Some(Instant::now())),
                        };
                    }
                    st.events.push_back(ev);
                }
                Err(e) => {
                    log!("sfu: poll_output: {e}");
                    st.rtc.disconnect();
                    return;
                }
            }
        }
    }

    /// Delivers queued events; caller holds `dispatch`.
    fn deliver(self: &Arc<Self>) {
        let Some(h) = self.handler.upgrade() else { return };
        loop {
            let ev = lock(&self.rtc).events.pop_front();
            match ev {
                Some(ev) => h.on_event(self, ev),
                None => break,
            }
        }
        let dead = {
            let st = lock(&self.rtc);
            !st.rtc.is_alive() || st.unhealthy_since.is_some_and(|t| t.elapsed() >= ICE_FAIL_AFTER)
        };
        if dead && !self.closed.swap(true, Ordering::AcqRel) {
            self.unregister();
            self.timer.notify_one();
            h.on_closed(self);
        }
    }

    /// Re-registers the local ufrag after renegotiation (an ICE restart mints
    /// new credentials).
    pub fn refresh_ufrag(self: &Arc<Self>) {
        let current = lock(&self.rtc).rtc.direct_api().local_ice_credentials().ufrag;
        let mut ufrag = lock(&self.ufrag);
        if *ufrag != current {
            let mut map = self.socket.ufrags.write().unwrap();
            map.remove(&*ufrag);
            map.insert(current.clone(), self.me.clone());
            *ufrag = current;
        }
    }

    #[cfg(test)]
    pub fn routes_ufrag(self: &Arc<Self>, ufrag: &str) -> bool {
        self.socket.ufrags.read().unwrap().get(ufrag).is_some_and(|w| std::ptr::eq(w.as_ptr(), Arc::as_ptr(self)))
    }

    /// Tears the PC down; `on_closed` fires once.
    pub fn close(self: &Arc<Self>) {
        if self.is_closed() {
            return;
        }
        self.with(|rtc| rtc.disconnect());
    }

    fn unregister(&self) {
        let remotes = std::mem::take(&mut lock(&self.rtc).remotes);
        let mut routes = self.socket.routes.write().unwrap();
        for r in remotes {
            if routes.get(&r).is_some_and(|w| std::ptr::eq(w.as_ptr(), self)) {
                routes.remove(&r);
            }
        }
        drop(routes);
        self.socket.ufrags.write().unwrap().remove(&*lock(&self.ufrag));
    }

    async fn timer_loop(self: Arc<Self>) {
        // Also wakes periodically so the ICE-failure watchdog runs even when
        // str0m has nothing scheduled.
        const WATCHDOG: Duration = Duration::from_secs(1);
        loop {
            if self.is_closed() {
                return;
            }
            let ns = self.deadline.load(Ordering::Acquire);
            let at =
                if ns == 0 { Instant::now() + WATCHDOG } else { (self.epoch + Duration::from_nanos(ns)).min(Instant::now() + WATCHDOG) };
            tokio::select! {
                _ = tokio::time::sleep_until(at.into()) => {}
                _ = self.timer.notified() => {}
            }
            if self.is_closed() {
                return;
            }
            let now = Instant::now();
            self.with(|rtc| {
                if let Err(e) = rtc.handle_input(Input::Timeout(now)) {
                    log!("sfu: timeout input: {e}");
                    rtc.disconnect();
                }
            });
        }
    }
}

impl Drop for Pc {
    fn drop(&mut self) {
        self.unregister();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stun_username_routing() {
        // Binding request with USERNAME "abcd:efgh".
        let mut m = vec![0x00, 0x01, 0x00, 0x0c, 0x21, 0x12, 0xA4, 0x42];
        m.extend_from_slice(&[0u8; 12]);
        m.extend_from_slice(&[0x00, 0x06, 0x00, 0x09]);
        m.extend_from_slice(b"abcd:efgh\0\0\0");
        m[3] = 16;
        assert_eq!(stun_local_ufrag(&m), Some("abcd"));
        assert_eq!(stun_local_ufrag(&[0x80, 1, 2, 3]), None);
    }
}

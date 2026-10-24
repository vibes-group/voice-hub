//! Per-codec layer parsing (AV1 via the DD extension, VP9 via its RTP payload
//! descriptor) and the DD chain-integrity gate used by the forward path.

use super::dd::{self, Descriptor};
use super::protocol::ScreenVideoCodec;

pub enum Adapter {
    Av1(dd::Parser),
    Vp9,
}

impl Adapter {
    pub fn new(codec: ScreenVideoCodec) -> Adapter {
        match codec {
            ScreenVideoCodec::Av1 => Adapter::Av1(dd::Parser::default()),
            ScreenVideoCodec::Vp9 => Adapter::Vp9,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Adapter::Av1(_) => "av1",
            Adapter::Vp9 => "vp9",
        }
    }

    /// `Ok(None)`: no layer info on this packet; forward permissively.
    pub fn parse(&mut self, payload: &[u8], dd_ext: Option<&[u8]>) -> Result<Option<Descriptor>, String> {
        match self {
            Adapter::Av1(p) => match dd_ext {
                Some(ext) => p.parse(ext).map_err(|e| e.to_string()),
                None => Ok(None),
            },
            Adapter::Vp9 => parse_vp9(payload).map(Some),
        }
    }
}

/// VP9 RTP payload descriptor (RFC 8741). First byte: I P L F B E V Z. VP9
/// has no chains, so the tracker degrades to keyframe-gated bootstrap while
/// TID still drives temporal-layer drops.
pub fn parse_vp9(p: &[u8]) -> Result<Descriptor, String> {
    const SHORT: &str = "vp9: payload too short";
    let head = *p.first().ok_or(SHORT)?;
    let (i, pb, l, f, b, e) = (head & 0x80 != 0, head & 0x40 != 0, head & 0x20 != 0, head & 0x10 != 0, head & 0x08 != 0, head & 0x04 != 0);
    let mut off = 1;
    let mut picture_id = 0u16;
    if i {
        let first = *p.get(off).ok_or(SHORT)?;
        if first & 0x80 != 0 {
            let second = *p.get(off + 1).ok_or(SHORT)?;
            picture_id = ((first & 0x7f) as u16) << 8 | second as u16;
            off += 2;
        } else {
            picture_id = (first & 0x7f) as u16;
            off += 1;
        }
    }
    let (mut tid, mut sid) = (0, 0);
    if l {
        let layers = *p.get(off).ok_or(SHORT)?;
        tid = (layers >> 5) & 0x07;
        sid = (layers >> 1) & 0x07;
        off += 1;
        if !f {
            p.get(off).ok_or(SHORT)?; // TL0PICIDX
        }
    }
    Ok(Descriptor {
        frame_number: picture_id,
        temporal_layer: tid,
        spatial_layer: sid,
        // !P and B: the first packet of a keyframe; later packets of the same
        // keyframe must not re-arm the tracker.
        is_keyframe: !pb && b,
        is_last_in_frame: e,
        ..Default::default()
    })
}

/// DD chain integrity for one subscriber. A frame in chain c carries
/// chain_diff[c] back to its predecessor; if that predecessor was not
/// forwarded, decodability is broken until the next keyframe, and forwarding
/// stops instead of feeding the decoder garbage. A 256-bit ring tracks
/// forwarded frame numbers over a 128-frame window.
pub struct ChainTracker {
    chain: usize,
    seen: [u8; 32],
    pub broken: bool,
    bootstrapped: bool,
}

impl ChainTracker {
    pub fn new(chain: usize) -> ChainTracker {
        ChainTracker { chain, seen: [0; 32], broken: false, bootstrapped: false }
    }

    /// A new target layer invalidates the window built for the old chain.
    pub fn set_chain(&mut self, chain: usize) {
        if chain != self.chain {
            self.chain = chain;
            self.reset();
        }
    }

    fn reset(&mut self) {
        self.seen = [0; 32];
        self.broken = false;
        self.bootstrapped = false;
    }

    /// `None` (no layer info) is permissive.
    pub fn allow(&mut self, desc: Option<&Descriptor>) -> bool {
        let Some(d) = desc else { return true };
        if d.attaches_structure || d.is_keyframe {
            self.reset();
            if d.is_keyframe {
                self.bootstrapped = true;
                self.mark(d.frame_number);
                return true;
            }
        }
        if !self.bootstrapped {
            // Attached mid-stream: drop until a keyframe (PLI'd by the SFU).
            self.broken = true;
            return false;
        }
        if self.broken {
            return false;
        }
        if let Some(&diff) = d.chain_diffs.get(self.chain)
            && diff > 0
            && !self.was_seen(d.frame_number.wrapping_sub(diff as u16))
        {
            self.broken = true;
            return false;
        }
        self.mark(d.frame_number);
        true
    }

    fn mark(&mut self, fn_: u16) {
        let expired = fn_.wrapping_sub(128) & 0xff;
        self.seen[(expired / 8) as usize] &= !(1 << (expired % 8));
        let f = fn_ & 0xff;
        self.seen[(f / 8) as usize] |= 1 << (f % 8);
    }

    fn was_seen(&self, fn_: u16) -> bool {
        let f = fn_ & 0xff;
        self.seen[(f / 8) as usize] & (1 << (f % 8)) != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(frame_number: u16, is_keyframe: bool, chain_diffs: &[u8]) -> Descriptor {
        Descriptor { frame_number, is_keyframe, chain_diffs: chain_diffs.to_vec(), ..Default::default() }
    }

    #[test]
    fn chain_tracker() {
        let mut ct = ChainTracker::new(0);
        assert!(ct.allow(None));
        assert!(!ct.allow(Some(&d(10, false, &[1]))));
        assert!(ct.broken);
        assert!(ct.allow(Some(&d(11, true, &[]))));
        assert!(!ct.broken);

        let mut ct = ChainTracker::new(0);
        ct.allow(Some(&d(100, true, &[0])));
        for f in 101..110 {
            assert!(ct.allow(Some(&d(f, false, &[1]))), "frame {f}");
        }

        let mut ct = ChainTracker::new(0);
        ct.allow(Some(&d(100, true, &[0])));
        assert!(!ct.allow(Some(&d(101, false, &[2]))));
        assert!(!ct.allow(Some(&d(102, false, &[1]))));
        assert!(ct.allow(Some(&d(103, true, &[0]))));
    }

    #[test]
    fn chain_tracker_resets() {
        let mut ct = ChainTracker::new(0);
        ct.allow(Some(&d(50, true, &[0])));
        ct.allow(Some(&d(51, false, &[1])));
        let structure = Descriptor { frame_number: 52, attaches_structure: true, chain_diffs: vec![1], ..Default::default() };
        assert!(!ct.allow(Some(&structure)));
        assert!(ct.allow(Some(&d(53, true, &[0]))));

        let mut ct = ChainTracker::new(0);
        ct.allow(Some(&d(1, true, &[0, 0])));
        ct.allow(Some(&d(2, false, &[1, 1])));
        ct.set_chain(1);
        assert!(!ct.allow(Some(&d(3, false, &[1, 1]))));

        let mut ct = ChainTracker::new(5);
        ct.allow(Some(&d(1, true, &[0])));
        assert!(ct.allow(Some(&d(2, false, &[1]))), "out-of-range chain is permissive");
    }

    #[test]
    fn vp9() {
        assert!(parse_vp9(&[]).is_err());
        let k = parse_vp9(&[0x08 | 0x04]).unwrap();
        assert!(k.is_keyframe && k.is_last_in_frame && k.temporal_layer == 0 && k.frame_number == 0);
        assert!(!parse_vp9(&[0x40 | 0x08]).unwrap().is_keyframe);
        let mid = parse_vp9(&[0x00]).unwrap();
        assert!(!mid.is_keyframe && !mid.is_last_in_frame);
        assert_eq!(parse_vp9(&[0xC0, 0x42]).unwrap().frame_number, 0x42);
        assert_eq!(parse_vp9(&[0xC0, 0x80 | 0x12, 0x34]).unwrap().frame_number, 0x1234);
        let l = parse_vp9(&[0xE0, 0x05, 2 << 5, 0x10]).unwrap();
        assert_eq!((l.temporal_layer, l.spatial_layer), (2, 0));
        let fl = parse_vp9(&[0xF0, 0x07, (1 << 5) | (2 << 1)]).unwrap();
        assert_eq!((fl.temporal_layer, fl.spatial_layer), (1, 2));
        assert!(parse_vp9(&[0x80]).is_err());
        assert!(parse_vp9(&[0x80, 0x80]).is_err());
        assert!(parse_vp9(&[0x20, 0x00]).is_err());
    }
}

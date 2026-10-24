//! AV1 Dependency Descriptor RTP header extension parser (parsing only).
//! Spec: https://aomediacodec.github.io/av1-rtp-spec/#dependency-descriptor-rtp-header-extension
//!
//! A parser keeps the running FrameDependencyStructure across packets: the
//! first packet of a stream attaches it, later packets reference templates by
//! ID. One parser per screen-share session, driven from its forward path.

pub const RTP_EXTENSION_URI: &str = "https://aomediacodec.github.io/av1-rtp-spec/#dependency-descriptor-rtp-header-extension";

/// template_id is 6 bits.
const MAX_TEMPLATES: usize = 64;

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Eof,
    NoStructure,
    InvalidTemplate,
    TooManyTemplates,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Error::Eof => "dd: not enough bits",
            Error::NoStructure => "dd: no FrameDependencyStructure seen yet",
            Error::InvalidTemplate => "dd: template id out of range",
            Error::TooManyTemplates => "dd: structure exceeds 64 templates",
        })
    }
}

/// One DD-bearing packet, projected onto what the forward path uses.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Descriptor {
    pub frame_number: u16,
    pub temporal_layer: u8,
    /// No upstream dependencies; re-arms the chain tracker.
    pub is_keyframe: bool,
    pub attaches_structure: bool,
    /// Per chain: frame number minus the previous frame number in that chain.
    pub chain_diffs: Vec<u8>,
}

struct BitReader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl BitReader<'_> {
    fn remaining(&self) -> usize {
        self.buf.len() * 8 - self.pos
    }

    fn read(&mut self, mut n: usize) -> Result<u32, Error> {
        if n == 0 {
            return Ok(0);
        }
        if self.remaining() < n {
            return Err(Error::Eof);
        }
        let mut v = 0u32;
        while n > 0 {
            let off = self.pos % 8;
            let left = 8 - off;
            let take = left.min(n);
            let chunk = (self.buf[self.pos / 8] >> (left - take)) & ((1u16 << take) - 1) as u8;
            v = (v << take) | chunk as u32;
            self.pos += take;
            n -= take;
        }
        Ok(v)
    }

    fn bool(&mut self) -> Result<bool, Error> {
        Ok(self.read(1)? != 0)
    }

    /// AV1 non-symmetric unsigned value in [0, n-1] (spec §4.10.7 ns(n)).
    fn ns(&mut self, n: u32) -> Result<u32, Error> {
        if n <= 1 {
            return Ok(0);
        }
        let width = 32 - n.leading_zeros() as usize;
        let k = (1u32 << width) - n;
        let v = self.read(width - 1)?;
        if v < k {
            return Ok(v);
        }
        let extra = self.read(1)?;
        Ok((v << 1) + extra - k)
    }
}

struct Template {
    temporal: u8,
    fdiffs: Vec<u8>,
    chain_diffs: Vec<u8>,
}

struct Structure {
    template_id_offset: usize,
    num_decode_targets: usize,
    num_chains: usize,
    templates: Vec<Template>,
}

#[derive(Default)]
pub struct Parser {
    structure: Option<Structure>,
}

impl Parser {
    /// `Ok(None)` when the extension is absent on this packet (normal).
    pub fn parse(&mut self, ext: &[u8]) -> Result<Option<Descriptor>, Error> {
        if ext.is_empty() {
            return Ok(None);
        }
        let mut br = BitReader { buf: ext, pos: 0 };
        br.read(2)?; // start_of_frame, end_of_frame
        let template_id = br.read(6)? as usize;
        let frame_number = br.read(16)? as u16;

        let (mut active_dt_present, mut custom_dtis, mut custom_fdiffs, mut custom_chains) = (false, false, false, false);
        let mut attached = None;
        if ext.len() > 3 {
            let structure_present = br.bool()?;
            active_dt_present = br.bool()?;
            custom_dtis = br.bool()?;
            custom_fdiffs = br.bool()?;
            custom_chains = br.bool()?;
            if structure_present {
                attached = Some(parse_structure(&mut br)?);
            }
        }
        let attaches = attached.is_some();
        if let Some(s) = attached {
            self.structure = Some(s);
        }
        let s = self.structure.as_ref().ok_or(Error::NoStructure)?;
        if active_dt_present {
            br.read(s.num_decode_targets)?; // active_decode_targets_bitmask
        }
        let idx = (template_id + MAX_TEMPLATES - s.template_id_offset) % MAX_TEMPLATES;
        let tmpl = s.templates.get(idx).ok_or(Error::InvalidTemplate)?;

        if custom_dtis {
            for _ in 0..s.num_decode_targets {
                br.read(2)?;
            }
        }
        let fdiff_count = if custom_fdiffs {
            let mut n = 0;
            loop {
                let size = br.read(2)? as usize;
                if size == 0 {
                    break;
                }
                br.read(size * 4)?;
                n += 1;
            }
            n
        } else {
            tmpl.fdiffs.len()
        };
        let chain_diffs = if custom_chains {
            (0..s.num_chains).map(|_| br.read(8).map(|v| v as u8)).collect::<Result<Vec<_>, _>>()?
        } else {
            tmpl.chain_diffs.clone()
        };

        Ok(Some(Descriptor {
            frame_number,
            temporal_layer: tmpl.temporal,
            is_keyframe: fdiff_count == 0,
            attaches_structure: attaches,
            chain_diffs,
        }))
    }
}

/// template_dependency_structure(), AV1 RTP spec §A.2.
fn parse_structure(br: &mut BitReader) -> Result<Structure, Error> {
    let template_id_offset = br.read(6)? as usize;
    let num_decode_targets = br.read(5)? as usize + 1;
    let mut templates = Vec::new();
    let (mut spatial, mut temporal) = (0u8, 0u8);
    loop {
        if templates.len() >= MAX_TEMPLATES {
            return Err(Error::TooManyTemplates);
        }
        templates.push(Template { temporal, fdiffs: Vec::new(), chain_diffs: Vec::new() });
        let idc = br.read(2)?;
        match idc {
            1 => temporal += 1,
            2 => {
                spatial += 1;
                temporal = 0;
            }
            _ => {}
        }
        if idc == 3 || br.remaining() == 0 {
            break;
        }
    }
    let spatial_layers = spatial as usize + 1;
    for _ in 0..templates.len() * num_decode_targets {
        br.read(2)?; // decode target indications
    }
    for t in &mut templates {
        while br.bool()? {
            t.fdiffs.push(br.read(4)? as u8 + 1);
        }
    }
    let num_chains = br.ns(num_decode_targets as u32 + 1)? as usize;
    if num_chains > 0 {
        // decode_target_protected_by_chain: consumed; the tracker assumes
        // chain i protects decode target i (libwebrtc's L1T3/L3T3 layout).
        for _ in 0..num_decode_targets {
            br.ns(num_chains as u32)?;
        }
        for t in &mut templates {
            t.chain_diffs = (0..num_chains).map(|_| br.read(4).map(|v| v as u8)).collect::<Result<_, _>>()?;
        }
    }
    if br.bool()? {
        for _ in 0..spatial_layers {
            br.read(16)?;
            br.read(16)?;
        }
    }
    Ok(Structure { template_id_offset, num_decode_targets, num_chains, templates })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    const BOOTSTRAP: &str = "c1017280081485214eafffaaaa863cf0430c10c302afc0aaa0063c00430010c002a000a80006000040001d954926e082b04a0941b820ac1282503157f974000ca864330e222222eca8655304224230eca877530077004200ef008601df010d";

    #[test]
    fn empty_and_no_structure() {
        let mut p = Parser::default();
        assert_eq!(p.parse(&[]), Ok(None));
        assert!(p.parse(&[0, 0, 0]).is_err());
    }

    #[test]
    fn chrome_capture_corpus() {
        let corpus = [
            BOOTSTRAP,
            "86017340fc",
            "46017340fc",
            "c3017540fc",
            "88017640fc",
            "48017640fc",
            "c2017840fc",
            BOOTSTRAP,
            "860173",
            "460173",
            "8b0174",
            "0b0174",
            "0b0174",
            "c30175",
        ];
        let mut p = Parser::default();
        for (i, h) in corpus.iter().enumerate() {
            let d = p.parse(&hex(h)).unwrap_or_else(|e| panic!("packet {i}: {e}")).unwrap();
            assert_eq!(d.attaches_structure, *h == BOOTSTRAP, "packet {i}");
        }
    }

    #[test]
    fn structure_sticky() {
        let mut p = Parser::default();
        let b = p.parse(&hex(BOOTSTRAP)).unwrap().unwrap();
        assert!(b.attaches_structure && b.is_keyframe);
        let d = p.parse(&hex("86017340fc")).unwrap().unwrap();
        assert!(!d.attaches_structure);
    }

    #[test]
    fn ns_coding() {
        // ns(5): width 3, k = 3; values 0..2 use 2 bits, 3..4 use 3 bits.
        let mut br = BitReader { buf: &[0b1011_1100], pos: 0 };
        assert_eq!(br.ns(5), Ok(2)); // "10"
        assert_eq!(br.ns(5), Ok(4)); // "111" → (3<<1)+1-3
    }
}

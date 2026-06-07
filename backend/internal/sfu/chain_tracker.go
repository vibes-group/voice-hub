package sfu

import (
	"voice-hub/backend/internal/sfu/dd"
)

// ChainTracker enforces DD chain integrity for one subscriber's forward path.
//
// The AV1 DD spec assigns one "chain" per decode target. A frame in chain c
// carries chain_diff[c] = (frameNumber - prevFrameNumberInChainC). A subscriber
// following chain c can decode the current frame only if it received every
// frame in that chain back to a keyframe.
//
// SFUs use chain tracking to detect the moment a temporal-layer downgrade or
// a packet loss has broken decodability for one subscriber, then short-circuit
// forwarding until a new keyframe re-arms the chain. Without this gate, the
// subscriber would receive frames that the decoder discards anyway and would
// in many cases display garbled output while the decoder hunts for a sync
// point. With it, the subscriber gets a clean freeze + PLI + recovery.
//
// One ChainTracker is owned by one subscriber and is read/written only on the
// session's forwardVideo goroutine. Not safe for concurrent use.
//
// Invariants:
//   - chain identifies the chain id this subscriber follows (== temporal layer
//     for the standard L1T3 / L3T3 templates Chrome publishes, where decode
//     target i is protected by chain i).
//   - The 128-frame sliding bitmap tracks which frameNumbers this subscriber
//     has been forwarded. Chain diffs in DD templates are 4 bits (max 15) and
//     in custom chains 8 bits (max 255), so a 128-frame look-back catches the
//     typical L1T3 cadence (1 keyframe every ~60 frames) without false drops.
//   - On a structure-attached keyframe-bearing packet the tracker resets and
//     re-arms from scratch.
type ChainTracker struct {
	chain        int
	seen         [256 / 8]byte
	broken       bool
	bootstrapped bool
}

// NewChainTracker constructs a tracker following chain c. For L1T3, callers
// pass the subscriber's target temporal layer (0..2).
func NewChainTracker(c int) *ChainTracker {
	return &ChainTracker{chain: c}
}

// SetChain updates the followed chain. Callers invoke this when a subscriber
// pins a new temporal layer; the tracker resets its window because the prior
// "seen" set was relative to the old chain and would mis-classify the new
// one's look-backs.
func (t *ChainTracker) SetChain(c int) {
	if c == t.chain {
		return
	}
	t.chain = c
	t.reset()
}

func (t *ChainTracker) reset() {
	for i := range t.seen {
		t.seen[i] = 0
	}
	t.broken = false
	t.bootstrapped = false
}

// Allow decides whether to forward a packet to the subscriber. Returns true
// when the chain is intact (or there is no chain to check); false when the
// packet would punch a hole the subscriber's decoder can't recover from.
//
// nil desc is treated as permissive: codecs that don't carry DD (older VP9
// publishers, fallback paths) fall through with no layer enforcement.
//
// IsKeyframe re-arms the chain — the subscriber's decoder will reset on it,
// and any prior break is now safe to clear.
func (t *ChainTracker) Allow(desc *dd.Descriptor) bool {
	if desc == nil {
		return true
	}

	// Keyframe and/or structure-attached: both reset the tracker. A
	// structure-attached non-keyframe is rare but possible (active-decode-
	// targets bitmap can also flip mid-stream); in that case we drop until
	// the next keyframe arrives. We merge the two branches into one reset
	// so a packet that is BOTH structure-attached AND a keyframe (the
	// common case at stream bootstrap) doesn't double-reset and have its
	// bootstrapped flag clobbered between the two paths.
	if desc.AttachesStructure || desc.IsKeyframe {
		t.reset()
		if desc.IsKeyframe {
			t.bootstrapped = true
			t.mark(desc.FrameNumber)
			return true
		}
	}

	if !t.bootstrapped {
		// We haven't seen a keyframe yet — drop until one arrives. PLI from
		// the SFU forward path will trigger the publisher to send one. This
		// also covers the case where a subscriber attaches mid-stream.
		t.broken = true
		return false
	}

	if t.broken {
		return false
	}

	if t.chain < len(desc.ChainDiffs) {
		diff := uint16(desc.ChainDiffs[t.chain])
		if diff > 0 {
			need := desc.FrameNumber - diff
			if !t.wasSeen(need) {
				t.broken = true
				return false
			}
		}
	}

	t.mark(desc.FrameNumber)
	return true
}

// mark records that frameNumber fn was forwarded. It also clears the slot 128
// frames back so the 256-bit ring tracks roughly the most recent half-window
// without ever returning a stale hit from before that point.
func (t *ChainTracker) mark(fn uint16) {
	expired := fn - 128
	t.seen[(expired&0xff)/8] &^= 1 << ((expired & 0xff) % 8)
	t.seen[(fn&0xff)/8] |= 1 << ((fn & 0xff) % 8)
}

func (t *ChainTracker) wasSeen(fn uint16) bool {
	return t.seen[(fn&0xff)/8]&(1<<((fn&0xff)%8)) != 0
}

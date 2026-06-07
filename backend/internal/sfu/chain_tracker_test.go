package sfu

import (
	"testing"

	"voice-hub/backend/internal/sfu/dd"
)

func TestChainTrackerAllowsNilDescriptor(t *testing.T) {
	t.Parallel()

	ct := NewChainTracker(0)
	if !ct.Allow(nil) {
		t.Fatal("Allow(nil) = false, want true (permissive when no DD info)")
	}
}

func TestChainTrackerRequiresKeyframeBootstrap(t *testing.T) {
	t.Parallel()

	ct := NewChainTracker(0)
	// First non-keyframe before any keyframe → drop.
	if ct.Allow(&dd.Descriptor{FrameNumber: 10, IsKeyframe: false, ChainDiffs: []uint8{1}}) {
		t.Fatal("Allow before bootstrap should be false")
	}
	if !ct.broken {
		t.Fatal("Broken should be true after first pre-bootstrap drop")
	}
	// Keyframe re-arms.
	if !ct.Allow(&dd.Descriptor{FrameNumber: 11, IsKeyframe: true}) {
		t.Fatal("Allow on keyframe should be true")
	}
	if ct.broken {
		t.Fatal("Broken should clear on keyframe")
	}
}

func TestChainTrackerAllowsIntactChain(t *testing.T) {
	t.Parallel()

	ct := NewChainTracker(0)
	ct.Allow(&dd.Descriptor{FrameNumber: 100, IsKeyframe: true, ChainDiffs: []uint8{0}})
	for fn := uint16(101); fn < 110; fn++ {
		if !ct.Allow(&dd.Descriptor{FrameNumber: fn, ChainDiffs: []uint8{1}}) {
			t.Fatalf("frame %d: dropped intact chain packet", fn)
		}
	}
}

func TestChainTrackerBreaksOnGap(t *testing.T) {
	t.Parallel()

	ct := NewChainTracker(0)
	ct.Allow(&dd.Descriptor{FrameNumber: 100, IsKeyframe: true, ChainDiffs: []uint8{0}})
	// chain_diff=2 means previous expected frame in chain is fn-2 = 99 — never seen.
	if ct.Allow(&dd.Descriptor{FrameNumber: 101, ChainDiffs: []uint8{2}}) {
		t.Fatal("Allow should return false on missing chain predecessor")
	}
	if !ct.broken {
		t.Fatal("Broken should be true after chain miss")
	}
	// Subsequent non-keyframes stay broken.
	if ct.Allow(&dd.Descriptor{FrameNumber: 102, ChainDiffs: []uint8{1}}) {
		t.Fatal("subsequent packet should also be dropped while broken")
	}
	// Keyframe heals.
	if !ct.Allow(&dd.Descriptor{FrameNumber: 103, IsKeyframe: true, ChainDiffs: []uint8{0}}) {
		t.Fatal("keyframe should reset and forward")
	}
}

func TestChainTrackerStructureChangeResets(t *testing.T) {
	t.Parallel()

	ct := NewChainTracker(0)
	ct.Allow(&dd.Descriptor{FrameNumber: 50, IsKeyframe: true, ChainDiffs: []uint8{0}})
	ct.Allow(&dd.Descriptor{FrameNumber: 51, ChainDiffs: []uint8{1}})
	// Structure change forces re-bootstrap even though packet is mid-chain.
	if ct.Allow(&dd.Descriptor{FrameNumber: 52, AttachesStructure: true, ChainDiffs: []uint8{1}}) {
		t.Fatal("structure-bearing non-keyframe should drop until keyframe")
	}
	if !ct.Allow(&dd.Descriptor{FrameNumber: 53, IsKeyframe: true, ChainDiffs: []uint8{0}}) {
		t.Fatal("keyframe after structure change should reset and forward")
	}
}

func TestChainTrackerSetChainResets(t *testing.T) {
	t.Parallel()

	ct := NewChainTracker(0)
	ct.Allow(&dd.Descriptor{FrameNumber: 1, IsKeyframe: true, ChainDiffs: []uint8{0, 0}})
	ct.Allow(&dd.Descriptor{FrameNumber: 2, ChainDiffs: []uint8{1, 1}})
	ct.SetChain(1)
	// After SetChain, we need a new keyframe — even though chain 1 looked
	// intact, we lost continuity by switching mid-stream.
	if ct.Allow(&dd.Descriptor{FrameNumber: 3, ChainDiffs: []uint8{1, 1}}) {
		t.Fatal("after SetChain, mandatory keyframe before forward")
	}
}

// TestChainTrackerChainIndexOutOfRange covers the "subscriber following chain
// 2 against a structure that only declares 1 chain" edge — should not panic
// and should be permissive (we can't enforce what we don't see).
func TestChainTrackerChainIndexOutOfRange(t *testing.T) {
	t.Parallel()

	ct := NewChainTracker(5)
	ct.Allow(&dd.Descriptor{FrameNumber: 1, IsKeyframe: true, ChainDiffs: []uint8{0}})
	if !ct.Allow(&dd.Descriptor{FrameNumber: 2, ChainDiffs: []uint8{1}}) {
		t.Fatal("out-of-range chain should be permissive, not blocking")
	}
}

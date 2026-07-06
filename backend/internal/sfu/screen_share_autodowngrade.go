package sfu

import (
	"log"
	"time"
)

// autoDowngradeLoop polls every subscriber's lossPerMille once per
// autoDowngradePollInterval and applies hysteretic temporal-layer changes.
//
// The decision is delegated to evalAutoDowngrade so tests can drive it with
// a fake clock. Layer changes go through SetTargetTemp (already concurrency-
// safe) and a single PLI per session per tick — the publisher will issue one
// keyframe even if several subscribers crossed thresholds simultaneously.
func (r *Room) autoDowngradeLoop(session *ScreenShareSession) {
	t := time.NewTicker(autoDowngradePollInterval)
	defer t.Stop()
	for {
		select {
		case <-session.ctx.Done():
			return
		case now := <-t.C:
			r.runAutoDowngradeTick(session, now)
		}
	}
}

func (r *Room) runAutoDowngradeTick(session *ScreenShareSession, now time.Time) {
	if !session.supportsTemporalFiltering() {
		return
	}
	policy := policyForMode(session.Mode())
	subs := session.subscribersSnapshot()

	anyChange := false
	for _, sub := range subs {
		if evalAutoDowngrade(sub, now, session.PublisherID, policy) {
			anyChange = true
		}
	}
	if anyChange {
		session.requestKeyframe()
	}
}

// evalAutoDowngrade reads sub.lossPerMille, advances the hysteresis windows,
// and may call SetTargetTemp. Returns true when the target temporal layer
// actually changed (caller PLIs once per tick if any sub flipped).
//
// The pubID parameter is only used for log lines — passing it in keeps the
// function free of *Room and trivial to unit-test.
func evalAutoDowngrade(sub *screenSubscriber, now time.Time, pubID string, policy modePolicy) bool {
	loss := sub.lossPerMille.Load()
	target := sub.targetTemp.Load()
	nowNs := now.UnixNano()

	if loss >= policy.highLossPM {
		sub.lowLossSince.Store(0)
		if target <= policy.floorTemp {
			sub.highLossSince.Store(0)
			return false
		}
		since := sub.highLossSince.Load()
		if since == 0 {
			sub.highLossSince.Store(nowNs)
			return false
		}
		if time.Duration(nowNs-since) < policy.highWindow {
			return false
		}
		sub.SetTargetTemp(target - 1)
		sub.highLossSince.Store(0)
		log.Printf("sfu: auto-downgrade sub=%s pub=%s temp=%d→%d loss=%d‰",
			sub.peerID, pubID, target, target-1, loss)
		return true
	}

	if loss <= policy.lowLossPM {
		sub.highLossSince.Store(0)
		if target >= 2 {
			sub.lowLossSince.Store(0)
			return false
		}
		since := sub.lowLossSince.Load()
		if since == 0 {
			sub.lowLossSince.Store(nowNs)
			return false
		}
		if time.Duration(nowNs-since) < policy.lowWindow {
			return false
		}
		sub.SetTargetTemp(target + 1)
		sub.lowLossSince.Store(0)
		log.Printf("sfu: auto-upgrade sub=%s pub=%s temp=%d→%d loss=%d‰",
			sub.peerID, pubID, target, target+1, loss)
		return true
	}

	// Mid-band: reset both streaks so a future excursion has to re-prove itself.
	sub.highLossSince.Store(0)
	sub.lowLossSince.Store(0)
	return false
}

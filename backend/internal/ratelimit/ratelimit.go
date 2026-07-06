// Package ratelimit provides an in-memory, per-key token-bucket Limiter for the
// HTTP surface (one bucket per client IP). It is process-local — this app runs
// as a single process (embedded SFU, in-memory rooms), so nothing needs to be
// shared across instances or survive a restart; counters reset on restart,
// which is fine for throttling. Memory is bounded by evicting idle keys on
// access (past TTL, or the oldest once MaxKeys is reached) rather than by a
// background sweeper.
package ratelimit

import (
	"sync"
	"time"

	"golang.org/x/time/rate"
)

const (
	defaultTTL     = time.Hour
	defaultMaxKeys = 1 << 16
)

// Config tunes a Limiter. Rate and Burst are the token-bucket parameters; TTL is
// how long an idle key is kept before it may be evicted; MaxKeys caps the number
// of live keys (the oldest is evicted past the cap).
type Config struct {
	Rate    rate.Limit
	Burst   int
	TTL     time.Duration
	MaxKeys int
}

// Limiter is a keyed token bucket: one rate.Limiter per key, created on first
// use and evicted once idle.
type Limiter struct {
	cfg     Config
	mu      sync.Mutex
	buckets map[string]*bucket
	clock   func() time.Time
}

type bucket struct {
	lim      *rate.Limiter
	lastSeen time.Time
}

func New(cfg Config) *Limiter {
	if cfg.TTL <= 0 {
		cfg.TTL = defaultTTL
	}
	if cfg.MaxKeys <= 0 {
		cfg.MaxKeys = defaultMaxKeys
	}
	return &Limiter{cfg: cfg, buckets: make(map[string]*bucket), clock: time.Now}
}

// Allow reports whether an event for key is permitted now. When it is not, the
// second return is how long until a retry would succeed (suitable for a
// Retry-After header).
func (l *Limiter) Allow(key string) (bool, time.Duration) {
	now := l.clock()
	l.mu.Lock()
	b := l.bucketLocked(key, now)
	l.mu.Unlock()

	r := b.lim.ReserveN(now, 1)
	if !r.OK() {
		return false, 0
	}
	if delay := r.DelayFrom(now); delay > 0 {
		r.CancelAt(now)
		return false, delay
	}
	return true, 0
}

func (l *Limiter) bucketLocked(key string, now time.Time) *bucket {
	if b, ok := l.buckets[key]; ok {
		b.lastSeen = now
		return b
	}
	if len(l.buckets) >= l.cfg.MaxKeys {
		evictOldest(l.buckets, l.cfg.TTL, now)
	}
	b := &bucket{lim: rate.NewLimiter(l.cfg.Rate, l.cfg.Burst), lastSeen: now}
	l.buckets[key] = b
	return b
}

// evictOldest drops every bucket idle past ttl and, if that freed nothing, the
// single oldest bucket — guaranteeing room for one more under the cap. Caller
// holds the lock.
func evictOldest(m map[string]*bucket, ttl time.Duration, now time.Time) {
	var oldestKey string
	var oldestSeen time.Time
	freed := false
	for k, b := range m {
		ls := b.lastSeen
		if now.Sub(ls) > ttl {
			delete(m, k)
			freed = true
			continue
		}
		if oldestKey == "" || ls.Before(oldestSeen) {
			oldestKey, oldestSeen = k, ls
		}
	}
	if !freed && oldestKey != "" {
		delete(m, oldestKey)
	}
}

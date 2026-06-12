package filestore

import (
	"errors"
	"os"
	"sync"
	"testing"
	"time"
)

// newTestStore builds a store rooted in a temp dir with the janitor effectively
// disabled (hour-long interval) so tests can drive sweeps manually.
func newTestStore(t *testing.T, cfg Config) *Store {
	t.Helper()
	cfg.TempDir = t.TempDir()
	if cfg.JanitorInterval == 0 {
		cfg.JanitorInterval = time.Hour
	}
	s, err := New(cfg)
	if err != nil {
		t.Fatalf("New: %v", err)
	}
	t.Cleanup(func() { _ = s.Close() })
	return s
}

func writeTemp(t *testing.T, s *Store, content string) (string, int64) {
	t.Helper()
	f, err := s.CreateTemp()
	if err != nil {
		t.Fatalf("CreateTemp: %v", err)
	}
	n, err := f.WriteString(content)
	if err != nil {
		t.Fatalf("write: %v", err)
	}
	if err := f.Close(); err != nil {
		t.Fatalf("close: %v", err)
	}
	return f.Name(), int64(n)
}

func TestStoreAndGet(t *testing.T) {
	t.Parallel()
	s := newTestStore(t, Config{TTL: time.Minute, TTLHardCap: time.Hour, MaxTotalBytes: 1 << 20})

	path, size := writeTemp(t, s, "hello")
	entry, err := s.Store("room1", "a.txt", "text/plain", size, path)
	if err != nil {
		t.Fatalf("Store: %v", err)
	}

	got, ok := s.Get(entry.UploadID)
	if !ok {
		t.Fatal("Get miss right after Store")
	}
	if got.RoomID != "room1" || got.Name != "a.txt" || got.MIME != "text/plain" || got.Size != size {
		t.Errorf("entry mismatch: %+v", got)
	}
	if _, ok := s.Get("does-not-exist"); ok {
		t.Error("Get on unknown id returned ok")
	}
}

func TestTTLEvictionAndBudgetRelease(t *testing.T) {
	t.Parallel()
	base := time.Unix(1_700_000_000, 0)
	s := newTestStore(t, Config{TTL: time.Minute, TTLHardCap: time.Hour, MaxTotalBytes: 1 << 20})
	s.now = func() time.Time { return base }

	path, size := writeTemp(t, s, "data")
	entry, err := s.Store("room1", "a", "text/plain", size, path)
	if err != nil {
		t.Fatalf("Store: %v", err)
	}

	// Past the TTL, Get reports the entry as gone even before a sweep runs.
	s.now = func() time.Time { return base.Add(2 * time.Minute) }
	if _, ok := s.Get(entry.UploadID); ok {
		t.Error("Get returned an expired entry")
	}

	s.sweep(base.Add(2 * time.Minute))
	if _, err := os.Stat(path); !os.IsNotExist(err) {
		t.Errorf("sweep did not remove temp file: stat err=%v", err)
	}
	s.mu.Lock()
	total, n := s.totalSize, len(s.entries)
	s.mu.Unlock()
	if total != 0 || n != 0 {
		t.Errorf("after sweep totalSize=%d entries=%d, want 0/0", total, n)
	}

	// Budget freed: a new entry that would previously not fit now stores fine.
	path2, size2 := writeTemp(t, s, "more")
	if _, err := s.Store("room1", "b", "text/plain", size2, path2); err != nil {
		t.Fatalf("Store after sweep: %v", err)
	}
}

func TestTouchCapsAtHardCap(t *testing.T) {
	t.Parallel()
	base := time.Unix(1_700_000_000, 0)
	s := newTestStore(t, Config{TTL: 10 * time.Minute, TTLHardCap: 20 * time.Minute, MaxTotalBytes: 1 << 20})
	s.now = func() time.Time { return base }

	path, size := writeTemp(t, s, "data")
	entry, err := s.Store("room1", "a", "text/plain", size, path)
	if err != nil {
		t.Fatalf("Store: %v", err)
	}

	// First touch well within the hard cap extends to now+TTL.
	s.now = func() time.Time { return base.Add(5 * time.Minute) }
	s.Touch(entry.UploadID)
	if got, _ := s.Get(entry.UploadID); !got.ExpiresAt.Equal(base.Add(15 * time.Minute)) {
		t.Errorf("after first touch ExpiresAt=%v, want %v", got.ExpiresAt, base.Add(15*time.Minute))
	}

	// Second touch would reach now+TTL = base+24m but is clamped to the
	// CreatedAt+TTLHardCap = base+20m ceiling.
	s.now = func() time.Time { return base.Add(14 * time.Minute) }
	s.Touch(entry.UploadID)
	got, ok := s.Get(entry.UploadID)
	if !ok {
		t.Fatal("entry expired unexpectedly")
	}
	if hardCap := base.Add(20 * time.Minute); !got.ExpiresAt.Equal(hardCap) {
		t.Errorf("after clamped touch ExpiresAt=%v, want hard cap %v", got.ExpiresAt, hardCap)
	}
}

func TestStoreBudgetExceeded(t *testing.T) {
	t.Parallel()
	s := newTestStore(t, Config{TTL: time.Minute, TTLHardCap: time.Hour, MaxTotalBytes: 10})

	p1, s1 := writeTemp(t, s, "12345")
	if _, err := s.Store("r", "a", "text/plain", s1, p1); err != nil {
		t.Fatalf("first Store: %v", err)
	}

	p2, s2 := writeTemp(t, s, "123456") // 5 + 6 = 11 > 10
	if _, err := s.Store("r", "b", "text/plain", s2, p2); !errors.Is(err, ErrBudgetExceeded) {
		t.Fatalf("second Store err=%v, want ErrBudgetExceeded", err)
	}

	s.mu.Lock()
	total := s.totalSize
	s.mu.Unlock()
	if total != s1 {
		t.Errorf("totalSize=%d after rejected Store, want %d", total, s1)
	}
}

func TestCloseRemovesWorkDir(t *testing.T) {
	t.Parallel()
	s, err := New(Config{TempDir: t.TempDir(), TTL: time.Minute, TTLHardCap: time.Hour, MaxTotalBytes: 1 << 20, JanitorInterval: time.Hour})
	if err != nil {
		t.Fatalf("New: %v", err)
	}
	workDir := s.workDir
	if _, err := os.Stat(workDir); err != nil {
		t.Fatalf("work dir missing after New: %v", err)
	}

	if err := s.Close(); err != nil {
		t.Fatalf("Close: %v", err)
	}
	if _, err := os.Stat(workDir); !os.IsNotExist(err) {
		t.Errorf("work dir still present after Close: stat err=%v", err)
	}
	if err := s.Close(); err != nil {
		t.Errorf("second Close: %v", err)
	}
}

func TestConcurrentStores(t *testing.T) {
	t.Parallel()
	s := newTestStore(t, Config{TTL: time.Minute, TTLHardCap: time.Hour, MaxTotalBytes: 1 << 30})

	const n = 50
	var wg sync.WaitGroup
	for i := 0; i < n; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			f, err := s.CreateTemp()
			if err != nil {
				t.Errorf("CreateTemp: %v", err)
				return
			}
			_, _ = f.WriteString("payload")
			_ = f.Close()
			entry, err := s.Store("r", "f", "application/octet-stream", 7, f.Name())
			if err != nil {
				t.Errorf("Store: %v", err)
				return
			}
			if _, ok := s.Get(entry.UploadID); !ok {
				t.Errorf("Get miss for %s", entry.UploadID)
			}
			s.Touch(entry.UploadID)
		}()
	}
	// Concurrent sweeps exercise the lock against stores/gets (nothing is
	// expired yet, so none are evicted).
	for i := 0; i < 5; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			s.sweep(s.now())
		}()
	}
	wg.Wait()

	s.mu.Lock()
	got := len(s.entries)
	s.mu.Unlock()
	if got != n {
		t.Errorf("entries=%d, want %d", got, n)
	}
}

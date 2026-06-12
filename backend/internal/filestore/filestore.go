// Package filestore is an in-process, disk-backed transient store for chat
// attachments. The server never persists attachments: an upload lands in a
// temp file with a short TTL, lives just long enough for the peers present at
// send time to download it, and is then swept. There is no knowledge of room
// membership here — eviction is purely TTL- and budget-driven, which keeps the
// store simple and correct under reconnects.
package filestore

import (
	"crypto/rand"
	"errors"
	"os"
	"sync"
	"time"

	"github.com/oklog/ulid/v2"
)

// ErrBudgetExceeded is returned by Store when accepting the upload would push
// the on-disk total past MaxTotalBytes. The caller owns the temp file until
// Store succeeds, so it must remove the file when this is returned.
var ErrBudgetExceeded = errors.New("filestore: total size budget exceeded")

// Entry is the metadata the store tracks for one transient upload.
type Entry struct {
	UploadID  string
	RoomID    string
	Name      string
	MIME      string
	Size      int64
	TempPath  string
	CreatedAt time.Time
	ExpiresAt time.Time
}

// Config tunes the transient store. Zero values for the durations and byte caps
// disable the corresponding limit (MaxTotalBytes <= 0 means unbounded).
type Config struct {
	// TempDir is the parent directory under which the store creates its own
	// working subdir.
	TempDir string
	// TTL is how long a fresh upload survives without a download.
	TTL time.Duration
	// TTLHardCap bounds the lifetime regardless of how often Touch extends it,
	// measured from CreatedAt.
	TTLHardCap time.Duration
	// MaxUploadBytes is the per-upload ceiling, enforced upstream; the store
	// itself only caps the total.
	MaxUploadBytes  int64
	MaxTotalBytes   int64
	JanitorInterval time.Duration
}

// Store holds the live transient entries and their on-disk backing files.
type Store struct {
	mu        sync.Mutex
	entries   map[string]*Entry
	totalSize int64

	cfg     Config
	workDir string

	stopJanitor chan struct{}
	janitorDone chan struct{}
	closeOnce   sync.Once

	// now is the clock; overridable in tests to drive TTL behaviour.
	now func() time.Time
}

// New creates the store's working directory and starts the background janitor.
func New(cfg Config) (*Store, error) {
	if cfg.JanitorInterval <= 0 {
		cfg.JanitorInterval = time.Minute
	}
	if err := os.MkdirAll(cfg.TempDir, 0o700); err != nil {
		return nil, err
	}
	workDir, err := os.MkdirTemp(cfg.TempDir, "voice-hub-uploads-")
	if err != nil {
		return nil, err
	}
	s := &Store{
		entries:     make(map[string]*Entry),
		cfg:         cfg,
		workDir:     workDir,
		stopJanitor: make(chan struct{}),
		janitorDone: make(chan struct{}),
		now:         time.Now,
	}
	go s.janitor()
	return s, nil
}

func (s *Store) MaxUploadBytes() int64 { return s.cfg.MaxUploadBytes }

// CreateTemp opens a new empty file inside the store's working directory for
// the caller to stream an upload into. The caller passes the resulting path to
// Store on success, or removes the file itself on failure.
func (s *Store) CreateTemp() (*os.File, error) {
	return os.CreateTemp(s.workDir, "up-*")
}

// Store registers a fully-written temp file under a fresh upload ID. It returns
// ErrBudgetExceeded (without taking ownership of tempPath) when the total
// on-disk budget would be exceeded.
func (s *Store) Store(roomID, name, mime string, size int64, tempPath string) (*Entry, error) {
	now := s.now()
	s.mu.Lock()
	defer s.mu.Unlock()

	if s.cfg.MaxTotalBytes > 0 && s.totalSize+size > s.cfg.MaxTotalBytes {
		return nil, ErrBudgetExceeded
	}

	entry := &Entry{
		UploadID:  ulid.MustNew(ulid.Timestamp(now), rand.Reader).String(),
		RoomID:    roomID,
		Name:      name,
		MIME:      mime,
		Size:      size,
		TempPath:  tempPath,
		CreatedAt: now,
		ExpiresAt: now.Add(s.cfg.TTL),
	}
	s.entries[entry.UploadID] = entry
	s.totalSize += size
	return entry, nil
}

// Get returns the entry for id, treating an expired-but-not-yet-swept entry as
// absent so downloads 404 the instant the TTL lapses.
func (s *Store) Get(id string) (*Entry, bool) {
	now := s.now()
	s.mu.Lock()
	defer s.mu.Unlock()
	entry, ok := s.entries[id]
	if !ok || now.After(entry.ExpiresAt) {
		return nil, false
	}
	return entry, true
}

// Touch extends an entry's TTL on download, never beyond CreatedAt+TTLHardCap
// and never shortening it. No-op for unknown or already-expired entries.
func (s *Store) Touch(id string) {
	now := s.now()
	s.mu.Lock()
	defer s.mu.Unlock()
	entry, ok := s.entries[id]
	if !ok || now.After(entry.ExpiresAt) {
		return
	}
	extended := now.Add(s.cfg.TTL)
	if hardCap := entry.CreatedAt.Add(s.cfg.TTLHardCap); extended.After(hardCap) {
		extended = hardCap
	}
	if extended.After(entry.ExpiresAt) {
		entry.ExpiresAt = extended
	}
}

// sweep removes every entry whose TTL has lapsed as of now, then deletes the
// backing files outside the lock.
func (s *Store) sweep(now time.Time) {
	s.mu.Lock()
	var stale []string
	for id, entry := range s.entries {
		if now.After(entry.ExpiresAt) {
			stale = append(stale, entry.TempPath)
			s.totalSize -= entry.Size
			delete(s.entries, id)
		}
	}
	s.mu.Unlock()

	for _, path := range stale {
		_ = os.Remove(path)
	}
}

func (s *Store) janitor() {
	defer close(s.janitorDone)
	ticker := time.NewTicker(s.cfg.JanitorInterval)
	defer ticker.Stop()
	for {
		select {
		case <-ticker.C:
			s.sweep(s.now())
		case <-s.stopJanitor:
			return
		}
	}
}

// Close stops the janitor and removes the entire working directory.
func (s *Store) Close() error {
	s.closeOnce.Do(func() {
		close(s.stopJanitor)
		<-s.janitorDone
	})
	return os.RemoveAll(s.workDir)
}

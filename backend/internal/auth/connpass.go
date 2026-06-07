package auth

import (
	"crypto/rand"
	"crypto/subtle"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"golang.org/x/crypto/argon2"
)

// argon2id parameters tuned for an interactive login: ~50ms on a modern core,
// 64 MiB memory. The connection password itself has ~140 bits of entropy,
// so the hash is mostly defense-in-depth in case the data file leaks.
const (
	argonTime    uint32 = 2
	argonMemory  uint32 = 64 * 1024 // KiB
	argonThreads uint8  = 1
	argonKeyLen  uint32 = 32
	argonSaltLen        = 16

	connPassFile = "connection-password.json"

	// MaxConnPassEntries caps the number of simultaneous connection passwords.
	MaxConnPassEntries = 16
	// MaxConnPassLabelLen caps user-supplied label length to keep the admin UI sane.
	MaxConnPassLabelLen = 64

	connPassFileVersion = 2
)

// ErrEntryNotFound is returned when an operation targets an unknown entry id.
var ErrEntryNotFound = errors.New("connpass: entry not found")

// ErrTooManyEntries is returned by Create when MaxConnPassEntries is already reached.
var ErrTooManyEntries = errors.New("connpass: too many entries")

// connPassDummyHash is burned on no-match so timing doesn't reveal "no entries" vs "wrong password".
var connPassDummyHash = func() string {
	h, _ := hashArgon2id("dummy-for-constant-time-verify")
	return h
}()

// connPassEntry is the on-disk representation of one connection password.
// Plaintext is never persisted.
type connPassEntry struct {
	ID         string    `json:"id"`
	Label      string    `json:"label"`
	Hash       string    `json:"hash"` // PHC-like: "argon2id$t=...$m=...$p=...$<salt>$<key>"
	Generation uint64    `json:"generation"`
	CreatedAt  time.Time `json:"created_at"`
	ExpiresAt  time.Time `json:"expires_at,omitzero"` // zero value = never expires
}

// IsExpired reports whether the entry has an expiry that is at or before now.
func (e connPassEntry) IsExpired(now time.Time) bool {
	return !e.ExpiresAt.IsZero() && !now.Before(e.ExpiresAt)
}

type connPassFileFormat struct {
	Version uint            `json:"version"`
	Entries []connPassEntry `json:"entries"`
}

// ConnPassStore guards the connection-password state and persists it to a JSON file.
// Safe for concurrent use.
type ConnPassStore struct {
	path string

	mu    sync.RWMutex
	state connPassFileFormat
}

// LoadConnPassStore reads connection-password.json from dir or initializes empty state.
func LoadConnPassStore(dir string) (*ConnPassStore, error) {
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return nil, fmt.Errorf("create data dir: %w", err)
	}
	s := &ConnPassStore{path: filepath.Join(dir, connPassFile), state: connPassFileFormat{Version: connPassFileVersion}}
	data, err := os.ReadFile(s.path)
	if errors.Is(err, os.ErrNotExist) {
		return s, nil
	}
	if err != nil {
		return nil, fmt.Errorf("read %s: %w", connPassFile, err)
	}
	if err := json.Unmarshal(data, &s.state); err != nil {
		return nil, fmt.Errorf("parse %s: %w", connPassFile, err)
	}
	return s, nil
}

// ConnPassEntryStatus is one row of the read-only snapshot returned to the admin UI.
type ConnPassEntryStatus struct {
	ID         string    `json:"id"`
	Label      string    `json:"label"`
	Generation uint64    `json:"generation"`
	CreatedAt  time.Time `json:"created_at"`
	ExpiresAt  time.Time `json:"expires_at,omitzero"`
	Expired    bool      `json:"expired"`
}

// ConnPassStatus is a read-only snapshot of all entries for the admin UI.
type ConnPassStatus struct {
	Entries []ConnPassEntryStatus `json:"entries"`
}

func (s *ConnPassStore) Status() ConnPassStatus {
	now := time.Now().UTC()
	s.mu.RLock()
	defer s.mu.RUnlock()
	out := ConnPassStatus{Entries: make([]ConnPassEntryStatus, 0, len(s.state.Entries))}
	for _, e := range s.state.Entries {
		out.Entries = append(out.Entries, ConnPassEntryStatus{
			ID:         e.ID,
			Label:      e.Label,
			Generation: e.Generation,
			CreatedAt:  e.CreatedAt,
			ExpiresAt:  e.ExpiresAt,
			Expired:    e.IsExpired(now),
		})
	}
	return out
}

// EntryGeneration returns the generation counter of the given entry. Returns
// (0, false) if no such entry exists OR the entry has expired — so user
// sessions tied to expired entries are rejected on the next request.
func (s *ConnPassStore) EntryGeneration(id string) (uint64, bool) {
	if id == "" {
		return 0, false
	}
	now := time.Now().UTC()
	s.mu.RLock()
	defer s.mu.RUnlock()
	for _, e := range s.state.Entries {
		if e.ID == id {
			if e.IsExpired(now) {
				return 0, false
			}
			return e.Generation, true
		}
	}
	return 0, false
}

// Verify checks whether plain matches any stored, non-expired entry. Returns the
// entry id and generation on first match. On no-match, burns one argon2id against
// connPassDummyHash so attackers cannot distinguish "wrong password" from "no entries" via timing.
func (s *ConnPassStore) Verify(plain string) (string, uint64, bool) {
	now := time.Now().UTC()
	s.mu.RLock()
	entries := make([]connPassEntry, len(s.state.Entries))
	copy(entries, s.state.Entries)
	s.mu.RUnlock()

	for _, e := range entries {
		if e.IsExpired(now) {
			continue
		}
		if verifyArgon2id(e.Hash, plain) {
			return e.ID, e.Generation, true
		}
	}
	_ = verifyArgon2id(connPassDummyHash, plain)
	return "", 0, false
}

// Create generates a new connection password entry with the given label and
// persists it. Returns the new entry's status plus the plaintext — caller must
// show it once and discard. Label is trimmed; an empty label is allowed.
// ttl=0 means the entry never expires; otherwise ExpiresAt = now + ttl.
func (s *ConnPassStore) Create(label string, ttl time.Duration) (ConnPassEntryStatus, string, error) {
	label = normalizeLabel(label)
	plain, err := GenerateConnPass()
	if err != nil {
		return ConnPassEntryStatus{}, "", err
	}
	hash, err := hashArgon2id(plain)
	if err != nil {
		return ConnPassEntryStatus{}, "", err
	}
	id, err := newEntryID()
	if err != nil {
		return ConnPassEntryStatus{}, "", err
	}

	now := time.Now().UTC()
	s.mu.Lock()
	if len(s.state.Entries) >= MaxConnPassEntries {
		s.mu.Unlock()
		return ConnPassEntryStatus{}, "", ErrTooManyEntries
	}
	entry := connPassEntry{
		ID:         id,
		Label:      label,
		Hash:       hash,
		Generation: 1,
		CreatedAt:  now,
		ExpiresAt:  expiryFor(now, ttl),
	}
	s.state.Entries = append(s.state.Entries, entry)
	snapshot := cloneState(s.state)
	s.mu.Unlock()
	if err := s.persist(snapshot); err != nil {
		return ConnPassEntryStatus{}, "", err
	}
	return statusOf(entry, now), plain, nil
}

// SetTTL updates the expiry without rotating the plaintext (extend, renew, or disable temporarily).
func (s *ConnPassStore) SetTTL(id string, ttl time.Duration) (ConnPassEntryStatus, error) {
	now := time.Now().UTC()
	s.mu.Lock()
	idx := indexOf(s.state.Entries, id)
	if idx < 0 {
		s.mu.Unlock()
		return ConnPassEntryStatus{}, ErrEntryNotFound
	}
	s.state.Entries[idx].ExpiresAt = expiryFor(now, ttl)
	entry := s.state.Entries[idx]
	snapshot := cloneState(s.state)
	s.mu.Unlock()
	if err := s.persist(snapshot); err != nil {
		return ConnPassEntryStatus{}, err
	}
	return statusOf(entry, now), nil
}

// expiryFor maps a TTL to an ExpiresAt timestamp:
//   - ttl == 0  → zero time (never expires)
//   - ttl  > 0  → now + ttl
//   - ttl  < 0  → in the past (entry is immediately disabled but kept on disk
//     so the admin can renew it later)
func expiryFor(now time.Time, ttl time.Duration) time.Time {
	if ttl == 0 {
		return time.Time{}
	}
	if ttl < 0 {
		return now.Add(-time.Second)
	}
	return now.Add(ttl)
}

func statusOf(e connPassEntry, now time.Time) ConnPassEntryStatus {
	return ConnPassEntryStatus{
		ID:         e.ID,
		Label:      e.Label,
		Generation: e.Generation,
		CreatedAt:  e.CreatedAt,
		ExpiresAt:  e.ExpiresAt,
		Expired:    e.IsExpired(now),
	}
}

// Rotate replaces the plaintext of the named entry and bumps its generation,
// invalidating any sessions that were issued against the old hash. The entry's
// existing ExpiresAt is preserved — use SetTTL to renew expiry.
func (s *ConnPassStore) Rotate(id string) (ConnPassEntryStatus, string, error) {
	plain, err := GenerateConnPass()
	if err != nil {
		return ConnPassEntryStatus{}, "", err
	}
	hash, err := hashArgon2id(plain)
	if err != nil {
		return ConnPassEntryStatus{}, "", err
	}

	now := time.Now().UTC()
	s.mu.Lock()
	idx := indexOf(s.state.Entries, id)
	if idx < 0 {
		s.mu.Unlock()
		return ConnPassEntryStatus{}, "", ErrEntryNotFound
	}
	s.state.Entries[idx].Hash = hash
	s.state.Entries[idx].Generation++
	s.state.Entries[idx].CreatedAt = now
	entry := s.state.Entries[idx]
	snapshot := cloneState(s.state)
	s.mu.Unlock()
	if err := s.persist(snapshot); err != nil {
		return ConnPassEntryStatus{}, "", err
	}
	return statusOf(entry, now), plain, nil
}

// Revoke removes the named entry. Sessions issued against it become
// unrecognised at the next request and are rejected.
func (s *ConnPassStore) Revoke(id string) error {
	s.mu.Lock()
	idx := indexOf(s.state.Entries, id)
	if idx < 0 {
		s.mu.Unlock()
		return ErrEntryNotFound
	}
	s.state.Entries = append(s.state.Entries[:idx], s.state.Entries[idx+1:]...)
	snapshot := cloneState(s.state)
	s.mu.Unlock()
	return s.persist(snapshot)
}

// Rename updates the label of the named entry.
func (s *ConnPassStore) Rename(id, label string) error {
	label = normalizeLabel(label)
	s.mu.Lock()
	idx := indexOf(s.state.Entries, id)
	if idx < 0 {
		s.mu.Unlock()
		return ErrEntryNotFound
	}
	s.state.Entries[idx].Label = label
	snapshot := cloneState(s.state)
	s.mu.Unlock()
	return s.persist(snapshot)
}

func indexOf(entries []connPassEntry, id string) int {
	for i, e := range entries {
		if e.ID == id {
			return i
		}
	}
	return -1
}

func cloneState(s connPassFileFormat) connPassFileFormat {
	out := connPassFileFormat{Version: connPassFileVersion, Entries: make([]connPassEntry, len(s.Entries))}
	copy(out.Entries, s.Entries)
	return out
}

func normalizeLabel(label string) string {
	label = strings.TrimSpace(label)
	if len(label) > MaxConnPassLabelLen {
		label = label[:MaxConnPassLabelLen]
	}
	return label
}

func newEntryID() (string, error) {
	buf := make([]byte, 6)
	if _, err := rand.Read(buf); err != nil {
		return "", err
	}
	return hex.EncodeToString(buf), nil
}

func (s *ConnPassStore) persist(state connPassFileFormat) error {
	state.Version = connPassFileVersion
	data, err := json.MarshalIndent(state, "", "  ")
	if err != nil {
		return err
	}
	return atomicWrite(s.path, data, 0o600)
}

func hashArgon2id(plain string) (string, error) {
	salt := make([]byte, argonSaltLen)
	if _, err := rand.Read(salt); err != nil {
		return "", err
	}
	key := argon2.IDKey([]byte(plain), salt, argonTime, argonMemory, argonThreads, argonKeyLen)
	return fmt.Sprintf(
		"argon2id$t=%d$m=%d$p=%d$%s$%s",
		argonTime, argonMemory, argonThreads,
		base64.RawStdEncoding.EncodeToString(salt),
		base64.RawStdEncoding.EncodeToString(key),
	), nil
}

func verifyArgon2id(encoded, plain string) bool {
	parts := strings.Split(encoded, "$")
	if len(parts) != 6 || parts[0] != "argon2id" {
		return false
	}
	var t, m uint32
	var p uint8
	if _, err := fmt.Sscanf(parts[1], "t=%d", &t); err != nil {
		return false
	}
	if _, err := fmt.Sscanf(parts[2], "m=%d", &m); err != nil {
		return false
	}
	if _, err := fmt.Sscanf(parts[3], "p=%d", &p); err != nil {
		return false
	}
	salt, err := base64.RawStdEncoding.DecodeString(parts[4])
	if err != nil {
		return false
	}
	want, err := base64.RawStdEncoding.DecodeString(parts[5])
	if err != nil {
		return false
	}
	got := argon2.IDKey([]byte(plain), salt, t, m, p, uint32(len(want)))
	return subtle.ConstantTimeCompare(got, want) == 1
}

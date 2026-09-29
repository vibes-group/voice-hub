package auth

import (
	"crypto/rand"
	"crypto/subtle"
	"encoding/base64"
	"fmt"
	"strings"

	"golang.org/x/crypto/argon2"
)

// argon2id parameters tuned for an interactive login: ~50ms on a modern core,
// 64 MiB memory. Connection passwords carry ~140 bits of entropy, so for them
// the hash is mostly defense-in-depth in case the data file leaks; the admin
// password is only as strong as the operator made it.
const (
	argonTime    uint32 = 2
	argonMemory  uint32 = 64 * 1024 // KiB
	argonThreads uint8  = 1
	argonKeyLen  uint32 = 32
	argonSaltLen        = 16
)

// HashPassword returns a PHC-like argon2id hash of plain:
// "argon2id$t=...$m=...$p=...$<salt>$<key>".
func HashPassword(plain string) (string, error) {
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

// VerifyPassword reports whether plain matches an encoded HashPassword hash.
// A malformed hash never matches.
func VerifyPassword(encoded, plain string) bool {
	h, ok := parseHash(encoded)
	if !ok {
		return false
	}
	got := argon2.IDKey([]byte(plain), h.salt, h.t, h.m, h.p, uint32(len(h.key)))
	return subtle.ConstantTimeCompare(got, h.key) == 1
}

// ValidPasswordHash reports whether encoded is a well-formed HashPassword hash.
// Lets startup reject a mangled env value instead of silently locking the admin out.
func ValidPasswordHash(encoded string) bool {
	_, ok := parseHash(encoded)
	return ok
}

type parsedHash struct {
	t, m      uint32
	p         uint8
	salt, key []byte
}

func parseHash(encoded string) (parsedHash, bool) {
	var h parsedHash
	parts := strings.Split(encoded, "$")
	if len(parts) != 6 || parts[0] != "argon2id" {
		return h, false
	}
	if _, err := fmt.Sscanf(parts[1], "t=%d", &h.t); err != nil || h.t == 0 {
		return h, false
	}
	if _, err := fmt.Sscanf(parts[2], "m=%d", &h.m); err != nil || h.m == 0 {
		return h, false
	}
	if _, err := fmt.Sscanf(parts[3], "p=%d", &h.p); err != nil || h.p == 0 {
		return h, false
	}
	var err error
	if h.salt, err = base64.RawStdEncoding.DecodeString(parts[4]); err != nil || len(h.salt) == 0 {
		return h, false
	}
	if h.key, err = base64.RawStdEncoding.DecodeString(parts[5]); err != nil || len(h.key) == 0 {
		return h, false
	}
	return h, true
}

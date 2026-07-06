package auth

import (
	"crypto/hmac"
	"crypto/sha256"
	"encoding/base64"
	"errors"
	"strconv"
	"strings"
	"time"
)

const CookieName = "vh_session"

// Role identifies the privilege level encoded in the session cookie.
type Role string

const (
	RoleAdmin Role = "admin"
	RoleUser  Role = "user"
)

type Session struct {
	Role         Role
	Generation   uint64 // ConnPass entry generation at issue time; user sessions invalidate when this drifts.
	AdminVersion string // Admin-password fingerprint at issue time; empty for user sessions.
	EntryID      string // ConnPass entry id at issue time; empty for admin sessions.
}

// AdminPasswordVersion returns a base64url HMAC fingerprint of password
// keyed by secret. Plaintext password is never stored in the cookie.
func AdminPasswordVersion(secret []byte, password string) string {
	mac := hmac.New(sha256.New, secret)
	mac.Write([]byte("admin-password:v1:"))
	mac.Write([]byte(password))
	return base64.RawURLEncoding.EncodeToString(mac.Sum(nil))
}

// Encode serializes a session as "<exp>:<role>:<gen>:<adminVer>:<entryID>.<sig>".
func Encode(secret []byte, role Role, gen uint64, adminVer, entryID string, ttl time.Duration) string {
	exp := time.Now().Add(ttl).Unix()
	payload := strconv.FormatInt(exp, 10) + ":" + string(role) + ":" + strconv.FormatUint(gen, 10) + ":" + adminVer + ":" + entryID
	mac := hmac.New(sha256.New, secret)
	mac.Write([]byte(payload))
	sig := base64.RawURLEncoding.EncodeToString(mac.Sum(nil))
	return payload + "." + sig
}

func Decode(secret []byte, value string) (Session, error) {
	idx := strings.LastIndexByte(value, '.')
	if idx < 0 {
		return Session{}, errors.New("malformed cookie")
	}
	payload, sig := value[:idx], value[idx+1:]
	mac := hmac.New(sha256.New, secret)
	mac.Write([]byte(payload))
	expected := base64.RawURLEncoding.EncodeToString(mac.Sum(nil))
	if !hmac.Equal([]byte(sig), []byte(expected)) {
		return Session{}, errors.New("bad signature")
	}
	parts := strings.SplitN(payload, ":", 5)
	if len(parts) != 5 {
		return Session{}, errors.New("malformed payload")
	}
	expUnix, err := strconv.ParseInt(parts[0], 10, 64)
	if err != nil {
		return Session{}, errors.New("bad expiry")
	}
	role := Role(parts[1])
	if role != RoleAdmin && role != RoleUser {
		return Session{}, errors.New("bad role")
	}
	gen, err := strconv.ParseUint(parts[2], 10, 64)
	if err != nil {
		return Session{}, errors.New("bad generation")
	}
	adminVer, entryID := parts[3], parts[4]
	expires := time.Unix(expUnix, 0)
	if time.Now().After(expires) {
		return Session{}, errors.New("expired")
	}
	return Session{Role: role, Generation: gen, AdminVersion: adminVer, EntryID: entryID}, nil
}

package config

import (
	"log"
	"net/netip"
	"os"
	"strconv"
	"time"
)

type Config struct {
	Addr          string
	WebDir        string
	AppHostname   string
	PublicIP      string
	TurnRealm     string
	AdminPassword string
	CookieSecure  bool
	UDPPortMin    uint16
	UDPPortMax    uint16
	TurnRelayMin  uint16
	TurnRelayMax  uint16
	// CIDR prefixes whose RemoteAddr is allowed to set X-Forwarded-For.
	// Default loopback-only; prod compose pins the docker network range.
	TrustedProxies []netip.Prefix
	// Transient chat-attachment store (see internal/filestore). Bytes are held
	// on disk only long enough to relay them to peers present at send time.
	UploadMaxBytes   int64
	UploadTempDir    string
	UploadTTL        time.Duration
	UploadTTLHardCap time.Duration
	UploadTotalBytes int64
	// Populated by main from disk after Load(); not env-backed.
	SessionSecret    []byte
	TurnSharedSecret string
}

func Load() (Config, error) {
	hostname := env("APP_HOSTNAME", "localhost")
	trusted, err := ParseTrustedProxies(os.Getenv("APP_TRUSTED_PROXIES"))
	if err != nil {
		return Config{}, err
	}
	return Config{
		Addr:           env("APP_ADDR", ":8080"),
		WebDir:         env("APP_WEB_DIR", "../frontend/dist"),
		AppHostname:    hostname,
		PublicIP:       os.Getenv("PUBLIC_IP"),
		TurnRealm:      env("TURN_REALM", hostname),
		AdminPassword:  os.Getenv("APP_ADMIN_PASSWORD"),
		CookieSecure:   envBool("APP_COOKIE_SECURE", true),
		UDPPortMin:     envUint16("UDP_PORT_MIN", 10101),
		UDPPortMax:     envUint16("UDP_PORT_MAX", 10200),
		TurnRelayMin:   envUint16("TURN_RELAY_PORT_MIN", 49160),
		TurnRelayMax:   envUint16("TURN_RELAY_PORT_MAX", 49199),
		TrustedProxies: trusted,

		UploadMaxBytes:   envInt64("UPLOAD_MAX_BYTES", 100<<20),
		UploadTempDir:    env("UPLOAD_TEMP_DIR", os.TempDir()),
		UploadTTL:        time.Duration(envInt64("UPLOAD_TTL_SECONDS", 300)) * time.Second,
		UploadTTLHardCap: time.Duration(envInt64("UPLOAD_TTL_HARD_CAP_SECONDS", 1800)) * time.Second,
		UploadTotalBytes: envInt64("UPLOAD_TOTAL_BYTES", 1<<30),
	}, nil
}

func env(key, fallback string) string {
	if value := os.Getenv(key); value != "" {
		return value
	}

	return fallback
}

func envBool(key string, fallback bool) bool {
	value := os.Getenv(key)
	if value == "" {
		return fallback
	}

	parsed, err := strconv.ParseBool(value)
	if err != nil {
		log.Printf("config: bad %s=%q (%v), using default %v", key, value, err, fallback)
		return fallback
	}

	return parsed
}

func envInt64(key string, fallback int64) int64 {
	value := os.Getenv(key)
	if value == "" {
		return fallback
	}

	parsed, err := strconv.ParseInt(value, 10, 64)
	if err != nil {
		log.Printf("config: bad %s=%q (%v), using default %d", key, value, err, fallback)
		return fallback
	}

	return parsed
}

func envUint16(key string, fallback uint16) uint16 {
	value := os.Getenv(key)
	if value == "" {
		return fallback
	}

	parsed, err := strconv.ParseUint(value, 10, 16)
	if err != nil {
		log.Printf("config: bad %s=%q (%v), using default %d", key, value, err, fallback)
		return fallback
	}

	return uint16(parsed)
}

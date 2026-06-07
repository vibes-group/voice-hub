package config

import (
	"log"
	"net/netip"
	"os"
	"strconv"
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

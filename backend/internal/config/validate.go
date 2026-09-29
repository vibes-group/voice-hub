package config

import (
	"errors"

	"voice-hub/backend/internal/auth"
)

// ValidateInsecureConfig refuses combinations that are only acceptable in a
// local dev container. allowInsecure is the boot-time `APP_ALLOW_INSECURE=1`
// override; it is intentionally not part of Config because it is a
// deployment-time guard, not a runtime tunable.
//
// Without this guard, a pure environment-variable mistake (running dev's
// env-config on a public host, or pointing prod-config at the dev admin
// password) silently produces an insecure deployment.
func ValidateInsecureConfig(cfg *Config, allowInsecure bool) error {
	if !cfg.CookieSecure && !allowInsecure {
		return errors.New("APP_COOKIE_SECURE=false requires APP_ALLOW_INSECURE=1")
	}
	if !allowInsecure && auth.VerifyPassword(cfg.AdminPasswordHash, "dev") {
		return errors.New("dev admin password requires APP_ALLOW_INSECURE=1")
	}
	return nil
}

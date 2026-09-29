package config

import (
	"testing"

	"voice-hub/backend/internal/auth"
)

func TestValidateInsecureConfig(t *testing.T) {
	realHash, err := auth.HashPassword("real-password")
	if err != nil {
		t.Fatal(err)
	}
	devHash, err := auth.HashPassword("dev")
	if err != nil {
		t.Fatal(err)
	}
	tests := []struct {
		name          string
		cfg           Config
		allowInsecure bool
		wantErr       bool
	}{
		{
			name:          "secure prod, no flag — ok",
			cfg:           Config{CookieSecure: true, AdminPasswordHash: realHash},
			allowInsecure: false,
			wantErr:       false,
		},
		{
			name:          "insecure cookie without flag — refused",
			cfg:           Config{CookieSecure: false, AdminPasswordHash: realHash},
			allowInsecure: false,
			wantErr:       true,
		},
		{
			name:          "insecure cookie with flag — ok",
			cfg:           Config{CookieSecure: false, AdminPasswordHash: realHash},
			allowInsecure: true,
			wantErr:       false,
		},
		{
			name:          "dev admin password without flag — refused",
			cfg:           Config{CookieSecure: true, AdminPasswordHash: devHash},
			allowInsecure: false,
			wantErr:       true,
		},
		{
			name:          "dev admin password with flag — ok",
			cfg:           Config{CookieSecure: true, AdminPasswordHash: devHash},
			allowInsecure: true,
			wantErr:       false,
		},
		{
			name:          "both insecure with flag — ok (matches dev compose)",
			cfg:           Config{CookieSecure: false, AdminPasswordHash: devHash},
			allowInsecure: true,
			wantErr:       false,
		},
	}
	for _, tc := range tests {
		t.Run(tc.name, func(t *testing.T) {
			err := ValidateInsecureConfig(&tc.cfg, tc.allowInsecure)
			if (err != nil) != tc.wantErr {
				t.Fatalf("err=%v wantErr=%v", err, tc.wantErr)
			}
		})
	}
}

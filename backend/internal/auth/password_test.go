package auth

import (
	"strings"
	"testing"
)

func TestHashPasswordRoundTrip(t *testing.T) {
	h, err := HashPassword("hunter2")
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(h, "hunter2") {
		t.Fatal("hash contains plaintext")
	}
	if !ValidPasswordHash(h) {
		t.Fatalf("own hash rejected: %q", h)
	}
	if !VerifyPassword(h, "hunter2") {
		t.Fatal("correct password rejected")
	}
	if VerifyPassword(h, "hunter3") {
		t.Fatal("wrong password accepted")
	}
	h2, _ := HashPassword("hunter2")
	if h == h2 {
		t.Fatal("two hashes of one password are equal — salt not random")
	}
}

func TestMalformedHashNeverMatches(t *testing.T) {
	for _, h := range []string{
		"",
		"hunter2",
		"argon2id$t=2$m=65536$p=1$c2FsdA",      // missing key
		"argon2id$t=0$m=65536$p=1$c2FsdA$a2V5", // zero rounds would panic argon2
		"argon2id$t=2$m=65536$p=0$c2FsdA$a2V5", // zero threads would panic argon2
		"$argon2id$v=19$m=65536,t=2,p=1$c2FsdA$a2V5", // foreign PHC layout
	} {
		if ValidPasswordHash(h) {
			t.Errorf("ValidPasswordHash(%q) = true", h)
		}
		if VerifyPassword(h, "") || VerifyPassword(h, "hunter2") {
			t.Errorf("VerifyPassword(%q) matched", h)
		}
	}
}

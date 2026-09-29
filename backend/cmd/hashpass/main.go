// Command hashpass prints the argon2id hash for APP_ADMIN_PASSWORD_HASH.
// The password comes from stdin so it stays out of argv and shell history:
//
//	read -rs P && printf %s "$P" | go run ./cmd/hashpass; unset P
package main

import (
	"fmt"
	"io"
	"os"
	"strings"

	"voice-hub/backend/internal/auth"
)

// The hash is brute-forceable offline by anyone who can read the host's .env,
// so a short password is only as safe as the host.
const minRecommendedLen = 16

func main() {
	if fi, err := os.Stdin.Stat(); err == nil && fi.Mode()&os.ModeCharDevice != 0 {
		fail(`pipe the password on stdin: read -rs P && printf %s "$P" | go run ./cmd/hashpass; unset P`)
	}
	buf, err := io.ReadAll(io.LimitReader(os.Stdin, 4096))
	if err != nil {
		fail("read stdin: " + err.Error())
	}
	plain := strings.TrimRight(string(buf), "\r\n")
	if plain == "" {
		fail("empty password")
	}
	if len(plain) < minRecommendedLen {
		fmt.Fprintf(os.Stderr, "warning: password is shorter than %d characters\n", minRecommendedLen)
	}
	hash, err := auth.HashPassword(plain)
	if err != nil {
		fail("hash: " + err.Error())
	}
	fmt.Println(hash)
}

func fail(msg string) {
	fmt.Fprintln(os.Stderr, "error: "+msg)
	os.Exit(1)
}

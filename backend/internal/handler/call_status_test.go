package handler

import (
	"net/http"
	"net/http/httptest"
	"testing"
)

func TestCallStatus(t *testing.T) {
	for _, tc := range []struct {
		name   string
		active bool
		want   string
	}{
		{name: "idle", want: "idle\n"},
		{name: "active", active: true, want: "active\n"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			rec := httptest.NewRecorder()
			CallStatus(func() bool { return tc.active }).ServeHTTP(rec, httptest.NewRequest(http.MethodGet, "/internal/call-status", nil))
			if rec.Code != http.StatusOK {
				t.Fatalf("status = %d, want %d", rec.Code, http.StatusOK)
			}
			if got := rec.Body.String(); got != tc.want {
				t.Fatalf("body = %q, want %q", got, tc.want)
			}
		})
	}
}

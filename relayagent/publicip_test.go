package main

import (
	"net/http"
	"net/http/httptest"
	"testing"
	"time"
)

func TestParsePublicIP(t *testing.T) {
	for _, tc := range []struct {
		in   string
		want string
		ok   bool
	}{
		{"203.0.113.7", "203.0.113.7", true},
		{"8.8.8.8", "8.8.8.8", true},
		{"2603:c022:8009:1f01::1", "2603:c022:8009:1f01::1", true},
		{"::ffff:203.0.113.7", "203.0.113.7", true},
		{"10.0.0.1", "", false},
		{"192.168.1.1", "", false},
		{"172.16.0.1", "", false},
		{"127.0.0.1", "", false},
		{"::1", "", false},
		{"0.0.0.0", "", false},
		{"fe80::1", "", false},
		{"224.0.0.1", "", false},
		{"fd00::1", "", false},
		{"not-an-ip", "", false},
		{"", "", false},
		{"203.0.113.7\n", "", false}, // trimming happens at the query layer
	} {
		got, ok := parsePublicIP(tc.in)
		if ok != tc.ok || got != tc.want {
			t.Errorf("parsePublicIP(%q) = (%q, %v), want (%q, %v)",
				tc.in, got, ok, tc.want, tc.ok)
		}
	}
}

// TestDetectPublicIPFallback: a failing endpoint and one returning
// garbage are skipped; the first valid IP wins. All failing is an
// error, not a garbage value.
func TestDetectPublicIPFallback(t *testing.T) {
	fail := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusInternalServerError)
	}))
	defer fail.Close()
	garbage := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte("<html>captive portal</html>\n"))
	}))
	defer garbage.Close()
	good := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte("203.0.113.7\n"))
	}))
	defer good.Close()
	client := &http.Client{Timeout: 5 * time.Second}

	ip, err := detectPublicIP(client, []string{fail.URL, garbage.URL, good.URL})
	if err != nil {
		t.Fatalf("detectPublicIP = error: %v", err)
	}
	if ip != "203.0.113.7" {
		t.Fatalf("detectPublicIP = %q, want 203.0.113.7", ip)
	}

	if _, err := detectPublicIP(client, []string{fail.URL, garbage.URL}); err == nil {
		t.Fatal("detectPublicIP with all endpoints failing should error")
	}
	if _, err := detectPublicIP(client, nil); err == nil {
		t.Fatal("detectPublicIP with no endpoints should error")
	}
}

// TestDetectorRefreshKeepsLast: a failed re-detect keeps the cached
// value; a successful one replaces it.
func TestDetectorRefreshKeepsLast(t *testing.T) {
	dir := t.TempDir()
	log := newLogger(dir, false)
	good := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		_, _ = w.Write([]byte("203.0.113.7\n"))
	}))
	defer good.Close()
	fail := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		w.WriteHeader(http.StatusInternalServerError)
	}))
	defer fail.Close()

	d := &publicIPDetector{
		endpoints: []string{good.URL},
		client:    &http.Client{Timeout: 5 * time.Second},
		interval:  time.Hour,
	}
	d.refresh(log)
	if d.Current() != "203.0.113.7" {
		t.Fatalf("Current() = %q, want 203.0.113.7", d.Current())
	}
	d.endpoints = []string{fail.URL}
	d.refresh(log)
	if d.Current() != "203.0.113.7" {
		t.Fatalf("Current() after failed refresh = %q, want 203.0.113.7", d.Current())
	}
}

// TestDetectorDisabled: an empty endpoint list (RELAY_PUBLIC_IP_URLS="")
// disables detection; refresh is a no-op and Current stays empty.
func TestDetectorDisabled(t *testing.T) {
	dir := t.TempDir()
	log := newLogger(dir, false)
	d := &publicIPDetector{client: &http.Client{Timeout: 5 * time.Second}}
	d.refresh(log)
	if d.Current() != "" {
		t.Fatalf("Current() = %q, want empty", d.Current())
	}
}

func TestPublicIPEndpointsEnvOverride(t *testing.T) {
	t.Setenv(publicIPEnvOverride, "http://127.0.0.1:9/ip, https://example.invalid/ip")
	got := publicIPEndpoints()
	if len(got) != 2 || got[0] != "http://127.0.0.1:9/ip" || got[1] != "https://example.invalid/ip" {
		t.Fatalf("publicIPEndpoints() = %v", got)
	}
	t.Setenv(publicIPEnvOverride, "")
	if got := publicIPEndpoints(); len(got) != 0 {
		t.Fatalf("publicIPEndpoints() with empty override = %v, want empty", got)
	}
}

// Copyright 2026 Contextual, Inc. https://agentrq.com
// This notice may not be modified or removed.
// SPDX-License-Identifier: AGPL-3.0-only

package clientip

import (
	"net"
	"net/http/httptest"
	"testing"
)

// X-Real-IP is believed only from a proxy on the host or the private network,
// which writes it itself; X-Forwarded-For is never read.
func TestProxied_BelievesOnlyAPrivateProxysRealIP(t *testing.T) {
	for name, tc := range map[string]struct {
		peer, realIP, want string
	}{
		"a public peer's own header is ignored":   {"203.0.113.7", "198.51.100.2", "203.0.113.7"},
		"a public IPv6 peer is named as it is":    {"2001:db8::1", "198.51.100.2", "2001:db8::1"},
		"a loopback proxy names the client":       {"127.0.0.1", "203.0.113.9", "203.0.113.9"},
		"an IPv6 loopback proxy is believed too":  {"::1", "203.0.113.9", "203.0.113.9"},
		"a private proxy names the client":        {"10.0.0.5", "203.0.113.9", "203.0.113.9"},
		"a private peer with no header is itself": {"172.16.0.3", "", "172.16.0.3"},
	} {
		if got := Proxied(net.ParseIP(tc.peer), tc.realIP); got != tc.want {
			t.Errorf("%s: Proxied named %q, want %q", name, got, tc.want)
		}
	}
}

func TestFromRequest(t *testing.T) {
	for name, tc := range map[string]struct {
		remote, realIP, xff, want string
	}{
		"a public peer is itself":                 {"203.0.113.7:4000", "", "", "203.0.113.7"},
		"a public peer's X-Real-IP is ignored":    {"203.0.113.7:4000", "198.51.100.2", "", "203.0.113.7"},
		"X-Forwarded-For is never read":           {"203.0.113.7:4000", "", "198.51.100.2", "203.0.113.7"},
		"not even from a private proxy":           {"10.0.0.5:4000", "", "198.51.100.2", "10.0.0.5"},
		"a private proxy's X-Real-IP is used":     {"10.0.0.5:4000", "203.0.113.9", "", "203.0.113.9"},
		"an IPv6 peer loses its port":             {"[2001:db8::1]:4000", "", "", "2001:db8::1"},
		"an address without a port is kept":       {"203.0.113.7", "", "", "203.0.113.7"},
		"a peer that is not an IP is named as is": {"pipe", "198.51.100.2", "", "pipe"},
	} {
		r := httptest.NewRequest("GET", "/", nil)
		r.RemoteAddr = tc.remote
		if tc.realIP != "" {
			r.Header.Set("X-Real-IP", tc.realIP)
		}
		if tc.xff != "" {
			r.Header.Set("X-Forwarded-For", tc.xff)
		}
		if got := FromRequest(r); got != tc.want {
			t.Errorf("%s: FromRequest named %q, want %q", name, got, tc.want)
		}
	}
}

// Copyright 2026 Contextual, Inc. https://agentrq.com
// This notice may not be modified or removed.
// SPDX-License-Identifier: AGPL-3.0-only

// Package clientip names the client a request came from, for every throttle
// that counts by address.
package clientip

import (
	"net"
	"net/http"
)

// Proxied believes X-Real-IP only from a proxy on a loopback or private
// address, which sets it itself; from anyone else it is whatever the client
// sent. X-Forwarded-For is never read: the client writes its first entries,
// and trusting them gave every try a fresh key.
func Proxied(peer net.IP, realIP string) string {
	if realIP != "" && (peer.IsLoopback() || peer.IsPrivate()) {
		return realIP
	}
	return peer.String()
}

// FromRequest is Proxied for a net/http request.
func FromRequest(r *http.Request) string {
	host, _, err := net.SplitHostPort(r.RemoteAddr)
	if err != nil {
		host = r.RemoteAddr
	}
	peer := net.ParseIP(host)
	if peer == nil {
		return host
	}
	return Proxied(peer, r.Header.Get("X-Real-IP"))
}

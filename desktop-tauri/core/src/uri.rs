// Copyright 2026 Contextual, Inc. https://agentrq.com
// This notice may not be modified or removed.
// SPDX-License-Identifier: AGPL-3.0-only

//! Percent-decoding with the semantics of JavaScript's `decodeURIComponent`.

/// Decode `%XX` escapes. `None` when an escape is malformed or the decoded
/// bytes are not UTF-8, which is where `decodeURIComponent` throws.
pub fn decode_uri_component(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3)?;
            if !hex.iter().all(u8::is_ascii_hexdigit) {
                return None;
            }
            out.push(u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_escapes() {
        assert_eq!(decode_uri_component("a%20b%2Bc").as_deref(), Some("a b+c"));
        assert_eq!(decode_uri_component("plain").as_deref(), Some("plain"));
        assert_eq!(decode_uri_component("%C3%A9").as_deref(), Some("é"));
    }

    #[test]
    fn refuses_a_malformed_escape() {
        assert_eq!(decode_uri_component("%zz"), None);
        assert_eq!(decode_uri_component("100%"), None);
        assert_eq!(decode_uri_component("%4"), None);
        assert_eq!(decode_uri_component("%+1"), None);
    }

    #[test]
    fn refuses_bytes_that_are_not_utf8() {
        assert_eq!(decode_uri_component("%FF"), None);
    }
}

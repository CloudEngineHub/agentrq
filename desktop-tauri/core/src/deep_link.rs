// Copyright 2026 Contextual, Inc. https://agentrq.com
// This notice may not be modified or removed.
// SPDX-License-Identifier: AGPL-3.0-only

//! Deep links: `agentrq://workspaces/<id>/tasks/<id>` becomes the in-app route
//! `/workspaces/<id>/tasks/<id>`.
//!
//! A deep link is untrusted input from outside the app, so the result is
//! checked against the routes that actually exist. Anything unrecognised is
//! `None`, and the caller opens the app at its default view.
//!
//! The scheme is a parameter: the Tauri build uses `agentrq-tauri` while it is
//! in preview, so it never competes with the Electron app for `agentrq://`.

use crate::uri::decode_uri_component;
use url::Url;

/// The scheme the Electron app registers.
pub const DEEP_LINK_SCHEME: &str = "agentrq";

/// The scheme the Tauri build registers during the preview.
pub const DEEP_LINK_SCHEME_TAURI: &str = "agentrq-tauri";

/// Top-level sections a link may address. These mirror the route table in
/// `frontend/src/app.js`; a link outside them has no destination.
pub const LINKABLE_SECTIONS: [&str; 4] = ["workspaces", "tasks", "events", "workflows"];

fn is_identifier(segment: &str) -> bool {
    !segment.is_empty()
        && segment
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Turn a deep-link URL into an in-app route, or `None` when the link is not
/// one we serve. `scheme` is given without the colon.
pub fn parse_deep_link(url: &str, scheme: &str) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    if parsed.scheme() != scheme {
        return None;
    }

    // `agentrq://workspaces/abc` parses with host `workspaces` and path `/abc`,
    // while `agentrq:///workspaces/abc` puts it all in the path. Both turn up in
    // the wild, so both are accepted.
    let raw = format!("{}{}", parsed.host_str().unwrap_or(""), parsed.path());
    let segments: Vec<String> = raw
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(|segment| decode_uri_component(segment).unwrap_or_else(|| segment.to_string()))
        .collect();

    let Some(section) = segments.first() else {
        return Some("/".to_string());
    };
    if !LINKABLE_SECTIONS.contains(&section.as_str()) {
        return None;
    }

    // Every remaining segment must be a plain identifier. This is external
    // input, and a segment carrying a slash, a traversal or a control character
    // has no business reaching the router.
    if !segments[1..].iter().all(|segment| is_identifier(segment)) {
        return None;
    }

    let search = match parsed.query() {
        Some(query) if !query.is_empty() => format!("?{query}"),
        _ => String::new(),
    };
    Some(format!("/{}{search}", segments.join("/")))
}

/// Find the deep link in a set of process arguments. Windows and Linux deliver
/// the link as an argv entry on the second launch, rather than through the
/// `open-url` event macOS uses.
pub fn deep_link_from_argv<S: AsRef<str>>(argv: &[S], scheme: &str) -> Option<String> {
    let prefix = format!("{scheme}://");
    argv.iter()
        .map(AsRef::as_ref)
        .find(|arg| arg.starts_with(&prefix))
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(url: &str) -> Option<String> {
        parse_deep_link(url, DEEP_LINK_SCHEME)
    }

    #[test]
    fn maps_a_workspace_link_onto_its_route() {
        assert_eq!(
            parse("agentrq://workspaces/0ZzhYQG2qtl").as_deref(),
            Some("/workspaces/0ZzhYQG2qtl")
        );
    }

    #[test]
    fn maps_a_task_link_onto_its_route() {
        assert_eq!(
            parse("agentrq://workspaces/0ZzhYQG2qtl/tasks/0hua6QI7nXN").as_deref(),
            Some("/workspaces/0ZzhYQG2qtl/tasks/0hua6QI7nXN")
        );
    }

    #[test]
    fn accepts_every_section_the_route_table_exposes() {
        for section in LINKABLE_SECTIONS {
            assert_eq!(
                parse(&format!("agentrq://{section}/abc")),
                Some(format!("/{section}/abc"))
            );
        }
    }

    #[test]
    fn accepts_the_triple_slash_form_a_link_can_also_take() {
        assert_eq!(
            parse("agentrq:///workspaces/abc").as_deref(),
            Some("/workspaces/abc")
        );
    }

    #[test]
    fn opens_the_default_view_for_a_bare_link() {
        assert_eq!(parse("agentrq://").as_deref(), Some("/"));
        assert_eq!(parse("agentrq:///").as_deref(), Some("/"));
    }

    #[test]
    fn keeps_a_query_string() {
        assert_eq!(
            parse("agentrq://tasks/all?filter=mine").as_deref(),
            Some("/tasks/all?filter=mine")
        );
    }

    #[test]
    fn decodes_percent_encoded_segments() {
        assert_eq!(
            parse("agentrq://workspaces/abc%2Ddef").as_deref(),
            Some("/workspaces/abc-def")
        );
    }

    #[test]
    fn refuses_another_scheme() {
        assert_eq!(parse("https://evil.example.com/workspaces/abc"), None);
        assert_eq!(parse("file:///etc/passwd"), None);
    }

    #[test]
    fn refuses_a_section_that_is_not_a_route() {
        assert_eq!(parse("agentrq://admin/abc"), None);
        assert_eq!(parse("agentrq://api/v1/auth/user"), None);
    }

    #[test]
    fn refuses_a_segment_that_is_not_a_plain_identifier() {
        assert_eq!(parse("agentrq://workspaces/..%2F..%2Fetc"), None);
        assert_eq!(parse("agentrq://workspaces/a%00b"), None);
        assert_eq!(parse("agentrq://workspaces/a b"), None);
    }

    #[test]
    fn refuses_anything_unparseable() {
        // null and undefined collapse to the empty string, as in JS.
        assert_eq!(parse("not a url"), None);
        assert_eq!(parse(""), None);
    }

    #[test]
    fn tolerates_a_malformed_escape_rather_than_throwing() {
        assert_eq!(parse("agentrq://workspaces/%zz"), None);
    }

    #[test]
    fn names_its_scheme() {
        assert_eq!(DEEP_LINK_SCHEME, "agentrq");
    }

    #[test]
    fn takes_the_scheme_as_a_parameter() {
        // Not in the vitest file: the Tauri build's preview scheme.
        assert_eq!(DEEP_LINK_SCHEME_TAURI, "agentrq-tauri");
        assert_eq!(
            parse_deep_link("agentrq-tauri://workspaces/abc", DEEP_LINK_SCHEME_TAURI).as_deref(),
            Some("/workspaces/abc")
        );
        assert_eq!(
            parse_deep_link("agentrq://workspaces/abc", DEEP_LINK_SCHEME_TAURI),
            None
        );
        assert_eq!(parse("agentrq-tauri://workspaces/abc"), None);
    }

    #[test]
    fn finds_the_link_windows_and_linux_pass_as_an_argument() {
        let argv = ["/path/to/AgentRQ", "--flag", "agentrq://workspaces/abc"];
        assert_eq!(
            deep_link_from_argv(&argv, DEEP_LINK_SCHEME).as_deref(),
            Some("agentrq://workspaces/abc")
        );
    }

    #[test]
    fn returns_nothing_when_there_is_no_link() {
        assert_eq!(
            deep_link_from_argv(&["/path/to/AgentRQ", "--flag"], DEEP_LINK_SCHEME),
            None
        );
        assert_eq!(deep_link_from_argv::<&str>(&[], DEEP_LINK_SCHEME), None);
    }

    #[test]
    fn ignores_non_string_arguments() {
        // JS skips null and numbers; argv here is already strings, so the
        // stand-ins are arguments that are not links.
        let argv = ["", "42", "agentrq://events"];
        assert_eq!(
            deep_link_from_argv(&argv, DEEP_LINK_SCHEME).as_deref(),
            Some("agentrq://events")
        );
    }
}

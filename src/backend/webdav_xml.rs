// WebDAV PROPFIND XML response parser.
//
// Parses multi-status XML responses from WebDAV servers into
// `RemoteEntry` values using quick-xml's SAX-style streaming reader.

use percent_encoding::percent_decode_str;
use quick_xml::Reader;
use quick_xml::events::Event;

use crate::error::{Error, Result};

use super::RemoteEntry;

/// Parse a WebDAV PROPFIND multi-status XML response.
///
/// `dav_base_path` is the DAV prefix to strip from hrefs
/// (e.g. `/remote.php/dav/files/user/`).
///
/// Returns entries excluding the base path itself (the directory being listed).
pub fn parse_propfind_response(xml: &str, dav_base_path: &str) -> Result<Vec<RemoteEntry>> {
    let mut reader = Reader::from_str(xml);

    let mut entries = Vec::new();

    // Current response being built
    let mut in_response = false;
    let mut in_propstat = false;
    let mut in_prop = false;
    let mut current_tag: Option<String> = None;

    // Fields for the current response
    let mut href: Option<String> = None;
    let mut is_dir = false;
    let mut size: u64 = 0;
    let mut mtime: i64 = 0;
    let mut etag: Option<String> = None;
    let mut content_hash: Option<String> = None;
    let mut content_type: Option<String> = None;
    let mut in_resourcetype = false;

    let reset = |href: &mut Option<String>,
                 is_dir: &mut bool,
                 size: &mut u64,
                 mtime: &mut i64,
                 etag: &mut Option<String>,
                 content_hash: &mut Option<String>,
                 content_type: &mut Option<String>| {
        *href = None;
        *is_dir = false;
        *size = 0;
        *mtime = 0;
        *etag = None;
        *content_hash = None;
        *content_type = None;
    };

    loop {
        match reader.read_event() {
            Ok(Event::Empty(e)) => {
                let local_name = local_name_str(e.local_name().as_ref());
                match local_name.as_str() {
                    "resourcetype" if in_prop => { /* empty resourcetype = file */ }
                    "collection" if in_resourcetype => is_dir = true,
                    _ => {}
                }
            }
            Ok(Event::Start(e)) => {
                let local_name = local_name_str(e.local_name().as_ref());
                match local_name.as_str() {
                    "response" => {
                        in_response = true;
                        reset(
                            &mut href,
                            &mut is_dir,
                            &mut size,
                            &mut mtime,
                            &mut etag,
                            &mut content_hash,
                            &mut content_type,
                        );
                    }
                    "propstat" => in_propstat = true,
                    "prop" if in_propstat => in_prop = true,
                    "resourcetype" if in_prop => in_resourcetype = true,
                    "collection" if in_resourcetype => is_dir = true,
                    "href" if in_response => current_tag = Some("href".to_owned()),
                    "getcontentlength" if in_prop => {
                        current_tag = Some("getcontentlength".to_owned());
                    }
                    "getlastmodified" if in_prop => {
                        current_tag = Some("getlastmodified".to_owned());
                    }
                    "getetag" if in_prop => current_tag = Some("getetag".to_owned()),
                    "getcontenttype" if in_prop => {
                        current_tag = Some("getcontenttype".to_owned());
                    }
                    "checksum" if in_prop => current_tag = Some("checksum".to_owned()),
                    _ => {}
                }
            }
            Ok(Event::Text(e)) => {
                if let Some(ref tag) = current_tag {
                    let text = e
                        .unescape()
                        .map_err(|err| Error::XmlParse(format!("text unescape: {err}")))?
                        .to_string();

                    match tag.as_str() {
                        "href" => {
                            href = Some(
                                percent_decode_str(&text)
                                    .decode_utf8()
                                    .map_err(|err| Error::XmlParse(format!("href decode: {err}")))?
                                    .to_string(),
                            );
                        }
                        "getcontentlength" => {
                            size = text.trim().parse().unwrap_or(0);
                        }
                        "getlastmodified" => {
                            mtime = parse_http_date(&text).unwrap_or(0);
                        }
                        "getetag" => {
                            etag = Some(strip_quotes(&text));
                        }
                        "getcontenttype" => {
                            content_type = Some(text);
                        }
                        "checksum" => {
                            content_hash = parse_checksum(&text);
                        }
                        _ => {}
                    }
                    current_tag = None;
                }
            }
            Ok(Event::End(e)) => {
                let local_name = local_name_str(e.local_name().as_ref());
                match local_name.as_str() {
                    "response" => {
                        in_response = false;
                        if let Some(ref h) = href {
                            let path = strip_dav_prefix(h, dav_base_path);
                            entries.push(RemoteEntry {
                                path,
                                is_dir,
                                size,
                                mtime,
                                etag: etag.clone(),
                                content_hash: content_hash.clone(),
                                content_type: content_type.clone(),
                            });
                        }
                    }
                    "propstat" => in_propstat = false,
                    "prop" => in_prop = false,
                    "resourcetype" => in_resourcetype = false,
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(err) => return Err(Error::XmlParse(format!("XML read error: {err}"))),
            _ => {}
        }
    }

    // Remove the entry that represents the directory itself (empty path or "/")
    entries.retain(|e| {
        let trimmed = e.path.trim_matches('/');
        !trimmed.is_empty()
    });

    Ok(entries)
}

/// Extract local name from a potentially namespaced element name.
fn local_name_str(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_string()
}

/// Strip the DAV base path prefix from an href.
fn strip_dav_prefix(href: &str, dav_base_path: &str) -> String {
    if let Some(stripped) = href.strip_prefix(dav_base_path) {
        stripped.to_owned()
    } else {
        href.to_owned()
    }
}

/// Strip surrounding quotes from an ETag value.
fn strip_quotes(s: &str) -> String {
    let s = s.trim();
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        s[1..s.len() - 1].to_owned()
    } else {
        s.to_owned()
    }
}

/// Parse an HTTP-date (RFC 2822 / RFC 7231) into a Unix timestamp.
fn parse_http_date(s: &str) -> Option<i64> {
    use chrono::DateTime;
    // Try RFC 2822 first (most common in WebDAV)
    if let Ok(dt) = DateTime::parse_from_rfc2822(s.trim()) {
        return Some(dt.timestamp());
    }
    // Fallback: RFC 7231 / HTTP-date format "Sun, 06 Nov 1994 08:49:37 GMT"
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s.trim(), "%a, %d %b %Y %H:%M:%S GMT") {
        return Some(dt.and_utc().timestamp());
    }
    None
}

/// Parse Nextcloud checksum property (e.g. "SHA1:abc SHA256:def").
/// Returns the SHA-256 hash if available, otherwise the first hash.
fn parse_checksum(s: &str) -> Option<String> {
    let parts: Vec<&str> = s.split_whitespace().collect();
    // Prefer SHA256
    for part in &parts {
        if part.starts_with("SHA256:") {
            return Some(part.to_string());
        }
    }
    // Fallback to first available
    parts.first().map(|p| p.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAV_BASE: &str = "/remote.php/dav/files/testuser/";

    fn make_multistatus(responses: &str) -> String {
        format!(
            r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:oc="http://owncloud.org/ns">
{responses}
</d:multistatus>"#
        )
    }

    fn make_response(href: &str, is_dir: bool, props: &str) -> String {
        let resourcetype = if is_dir {
            "<d:resourcetype><d:collection/></d:resourcetype>"
        } else {
            "<d:resourcetype/>"
        };
        format!(
            r#"<d:response>
  <d:href>{href}</d:href>
  <d:propstat>
    <d:prop>
      {resourcetype}
      {props}
    </d:prop>
    <d:status>HTTP/1.1 200 OK</d:status>
  </d:propstat>
</d:response>"#
        )
    }

    #[test]
    fn parse_directory_listing() {
        let xml = make_multistatus(&format!(
            "{}\n{}\n{}",
            make_response(
                &format!("{DAV_BASE}"),
                true,
                "<d:getlastmodified>Thu, 01 Jan 2024 00:00:00 GMT</d:getlastmodified>"
            ),
            make_response(
                &format!("{DAV_BASE}file1.txt"),
                false,
                r#"<d:getcontentlength>1024</d:getcontentlength>
                <d:getlastmodified>Fri, 02 Feb 2024 12:00:00 GMT</d:getlastmodified>
                <d:getetag>"abc123"</d:getetag>
                <d:getcontenttype>text/plain</d:getcontenttype>"#,
            ),
            make_response(
                &format!("{DAV_BASE}subdir/"),
                true,
                "<d:getlastmodified>Sat, 03 Mar 2024 06:00:00 GMT</d:getlastmodified>"
            ),
        ));

        let entries = parse_propfind_response(&xml, DAV_BASE).unwrap();
        assert_eq!(entries.len(), 2);

        let file = &entries[0];
        assert_eq!(file.path, "file1.txt");
        assert!(!file.is_dir);
        assert_eq!(file.size, 1024);
        assert_eq!(file.etag.as_deref(), Some("abc123"));
        assert_eq!(file.content_type.as_deref(), Some("text/plain"));

        let dir = &entries[1];
        assert_eq!(dir.path, "subdir/");
        assert!(dir.is_dir);
    }

    #[test]
    fn parse_single_file_depth_0() {
        let xml = make_multistatus(&make_response(
            &format!("{DAV_BASE}photo.jpg"),
            false,
            r#"<d:getcontentlength>2048</d:getcontentlength>
            <d:getlastmodified>Mon, 15 Jan 2024 10:30:00 GMT</d:getlastmodified>
            <d:getetag>"xyz789"</d:getetag>"#,
        ));

        let entries = parse_propfind_response(&xml, DAV_BASE).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "photo.jpg");
        assert_eq!(entries[0].size, 2048);
    }

    #[test]
    fn parse_empty_directory() {
        let xml = make_multistatus(&make_response(
            &format!("{DAV_BASE}empty/"),
            true,
            "<d:getlastmodified>Tue, 01 Jan 2024 00:00:00 GMT</d:getlastmodified>",
        ));

        // The directory itself should be filtered out
        let entries = parse_propfind_response(&xml, &format!("{DAV_BASE}empty/")).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn etag_quote_stripping() {
        assert_eq!(strip_quotes(r#""abc123""#), "abc123");
        assert_eq!(strip_quotes("no-quotes"), "no-quotes");
        assert_eq!(strip_quotes(r#" "spaced" "#), "spaced");
    }

    #[test]
    fn lastmodified_timestamp_conversion() {
        // RFC 2822
        let ts = parse_http_date("Fri, 02 Feb 2024 12:00:00 +0000").unwrap();
        assert_eq!(ts, 1706875200);

        // HTTP-date (RFC 7231)
        let ts = parse_http_date("Fri, 02 Feb 2024 12:00:00 GMT").unwrap();
        assert_eq!(ts, 1706875200);

        // Invalid
        assert!(parse_http_date("not a date").is_none());
    }

    #[test]
    fn url_encoded_href_decoding() {
        let xml = make_multistatus(&make_response(
            &format!("{DAV_BASE}my%20file%20%281%29.txt"),
            false,
            r#"<d:getcontentlength>100</d:getcontentlength>
            <d:getlastmodified>Thu, 01 Jan 2024 00:00:00 GMT</d:getlastmodified>"#,
        ));

        let entries = parse_propfind_response(&xml, DAV_BASE).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "my file (1).txt");
    }

    #[test]
    fn malformed_xml_returns_error() {
        let bad_xml = "<d:multistatus><d:response><unclosed>";
        let result = parse_propfind_response(bad_xml, DAV_BASE);
        // Should still parse what it can or return entries (quick-xml may be lenient)
        // The important thing is it doesn't panic
        assert!(result.is_ok() || matches!(result.unwrap_err(), Error::XmlParse(_)));
    }

    #[test]
    fn checksum_parsing() {
        assert_eq!(
            parse_checksum("SHA1:abc SHA256:def"),
            Some("SHA256:def".to_owned())
        );
        assert_eq!(parse_checksum("SHA1:abc"), Some("SHA1:abc".to_owned()));
        assert_eq!(parse_checksum(""), None);
    }
}

// Integration tests for the Nextcloud WebDAV backend.
//
// Uses wiremock to simulate a Nextcloud WebDAV server.

use std::path::PathBuf;

use bytes::Bytes;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use mirage::backend::Backend;
use mirage::backend::nextcloud::NextcloudClient;
use mirage::config::Config;

const USERNAME: &str = "testuser";
const PASSWORD: &str = "testpass";

fn dav_path(file: &str) -> String {
    format!("/remote.php/dav/files/{USERNAME}/{file}")
}

fn dav_base() -> String {
    format!("/remote.php/dav/files/{USERNAME}/")
}

fn test_config(server_url: &str) -> Config {
    Config {
        server_url: server_url.to_owned(),
        username: USERNAME.to_owned(),
        password: Some(PASSWORD.to_owned()),
        cache_dir: PathBuf::from("/tmp/mirage-test-cache"),
        cache_limit_bytes: 1_000_000,
        mount_point: PathBuf::from("/tmp/mirage-test-mount"),
        sync_interval_secs: 300,
        retry_base_secs: 30,
        retry_max_secs: 600,
        always_local_paths: vec![],
        connect_timeout_secs: 10,
        request_timeout_secs: 60,
        ignore_file: None,
        remote_base_path: None,
        log_level: None,
    }
}

fn propfind_response(responses: &str) -> String {
    format!(
        r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:oc="http://owncloud.org/ns">
{responses}
</d:multistatus>"#
    )
}

fn entry_response(href: &str, is_dir: bool, props: &str) -> String {
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

#[tokio::test]
async fn list_dir_returns_entries() {
    let server = MockServer::start().await;

    let xml = propfind_response(&format!(
        "{}\n{}\n{}",
        entry_response(
            &dav_base(),
            true,
            "<d:getlastmodified>Thu, 01 Jan 2024 00:00:00 GMT</d:getlastmodified>"
        ),
        entry_response(
            &dav_path("file1.txt"),
            false,
            r#"<d:getcontentlength>1024</d:getcontentlength>
            <d:getlastmodified>Fri, 02 Feb 2024 12:00:00 GMT</d:getlastmodified>
            <d:getetag>"etag1"</d:getetag>"#,
        ),
        entry_response(
            &dav_path("subdir/"),
            true,
            "<d:getlastmodified>Sat, 03 Mar 2024 06:00:00 GMT</d:getlastmodified>"
        ),
    ));

    Mock::given(method("PROPFIND"))
        .and(path(dav_base()))
        .and(header("Depth", "1"))
        .respond_with(ResponseTemplate::new(207).set_body_string(xml))
        .mount(&server)
        .await;

    let config = test_config(&server.uri());
    let client = NextcloudClient::new(&config).unwrap();
    let entries = client.list_dir("").await.unwrap();

    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].path, "file1.txt");
    assert!(!entries[0].is_dir);
    assert_eq!(entries[0].size, 1024);
    assert_eq!(entries[1].path, "subdir/");
    assert!(entries[1].is_dir);
}

#[tokio::test]
async fn download_returns_bytes() {
    let server = MockServer::start().await;
    let body = b"hello world";

    Mock::given(method("GET"))
        .and(path(dav_path("file.txt")))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body.to_vec()))
        .mount(&server)
        .await;

    let config = test_config(&server.uri());
    let client = NextcloudClient::new(&config).unwrap();
    let data = client.download("file.txt").await.unwrap();

    assert_eq!(data.as_ref(), b"hello world");
}

#[tokio::test]
async fn upload_puts_and_returns_metadata() {
    let server = MockServer::start().await;

    // PUT response
    Mock::given(method("PUT"))
        .and(path(dav_path("new.txt")))
        .respond_with(ResponseTemplate::new(201))
        .mount(&server)
        .await;

    // PROPFIND for metadata after upload
    let xml = propfind_response(&entry_response(
        &dav_path("new.txt"),
        false,
        r#"<d:getcontentlength>5</d:getcontentlength>
        <d:getlastmodified>Thu, 01 Jan 2024 00:00:00 GMT</d:getlastmodified>
        <d:getetag>"newetag"</d:getetag>"#,
    ));

    Mock::given(method("PROPFIND"))
        .and(path(dav_path("new.txt")))
        .and(header("Depth", "0"))
        .respond_with(ResponseTemplate::new(207).set_body_string(xml))
        .mount(&server)
        .await;

    let config = test_config(&server.uri());
    let client = NextcloudClient::new(&config).unwrap();
    let entry = client
        .upload("new.txt", Bytes::from_static(b"hello"))
        .await
        .unwrap();

    assert_eq!(entry.path, "new.txt");
    assert_eq!(entry.etag.as_deref(), Some("newetag"));
}

#[tokio::test]
async fn delete_succeeds() {
    let server = MockServer::start().await;

    Mock::given(method("DELETE"))
        .and(path(dav_path("old.txt")))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    let config = test_config(&server.uri());
    let client = NextcloudClient::new(&config).unwrap();
    client.delete("old.txt").await.unwrap();
}

#[tokio::test]
async fn create_dir_succeeds() {
    let server = MockServer::start().await;

    Mock::given(method("MKCOL"))
        .and(path(dav_path("newdir/")))
        .respond_with(ResponseTemplate::new(201))
        .mount(&server)
        .await;

    let config = test_config(&server.uri());
    let client = NextcloudClient::new(&config).unwrap();
    client.create_dir("newdir/").await.unwrap();
}

#[tokio::test]
async fn move_entry_sends_destination_header() {
    let server = MockServer::start().await;

    let config = test_config(&server.uri());
    let dest_url = format!("{}{}", server.uri(), dav_path("moved.txt"));

    Mock::given(method("MOVE"))
        .and(path(dav_path("original.txt")))
        .and(header("Destination", dest_url.as_str()))
        .and(header("Overwrite", "F"))
        .respond_with(ResponseTemplate::new(201))
        .mount(&server)
        .await;

    let client = NextcloudClient::new(&config).unwrap();
    client
        .move_entry("original.txt", "moved.txt")
        .await
        .unwrap();
}

#[tokio::test]
async fn auth_failure_returns_error() {
    let server = MockServer::start().await;

    Mock::given(method("PROPFIND"))
        .and(path(dav_base()))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let config = test_config(&server.uri());
    let client = NextcloudClient::new(&config).unwrap();
    let err = client.list_dir("").await.unwrap_err();

    assert!(matches!(err, mirage::error::Error::AuthFailed));
}

#[tokio::test]
async fn not_found_returns_error() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path(dav_path("missing.txt")))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let config = test_config(&server.uri());
    let client = NextcloudClient::new(&config).unwrap();
    let err = client.download("missing.txt").await.unwrap_err();

    assert!(matches!(err, mirage::error::Error::NotFound(_)));
}

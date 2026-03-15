// Nextcloud WebDAV backend.
//
// Implements cloud storage operations via WebDAV protocol:
// - List files (PROPFIND)
// - Download / Upload files (GET / PUT)
// - Delete / Rename (DELETE / MOVE)
// - Create directories (MKCOL)

use bytes::Bytes;
use reqwest::header::{CONTENT_TYPE, HeaderValue};
use reqwest::{Client, Method, StatusCode};
use secrecy::{ExposeSecret, SecretString};
use tracing::{debug, warn};

use crate::config::Config;
use crate::error::{Error, Result};

use super::webdav_xml::parse_propfind_response;
use super::{Backend, RemoteEntry};

/// WebDAV PROPFIND request body for retrieving all relevant properties.
const PROPFIND_BODY: &str = r#"<?xml version="1.0"?>
<d:propfind xmlns:d="DAV:" xmlns:oc="http://owncloud.org/ns">
  <d:prop>
    <d:resourcetype/>
    <d:getcontentlength/>
    <d:getlastmodified/>
    <d:getetag/>
    <d:getcontenttype/>
    <oc:checksums/>
  </d:prop>
</d:propfind>"#;

/// Nextcloud WebDAV client.
pub struct NextcloudClient {
    client: Client,
    dav_base_url: String,
    dav_base_path: String,
    username: String,
    password: SecretString,
}

impl NextcloudClient {
    /// Create a new Nextcloud client from application config.
    pub fn new(config: &Config) -> Result<Self> {
        let password = config.resolve_password()?;
        let client = Client::builder()
            .connect_timeout(std::time::Duration::from_secs(config.connect_timeout_secs))
            .timeout(std::time::Duration::from_secs(config.request_timeout_secs))
            .build()
            .map_err(|e| Error::Config(format!("failed to build HTTP client: {e}")))?;

        Ok(Self {
            client,
            dav_base_url: config.dav_base_url(),
            dav_base_path: config.dav_base_path(),
            username: config.username.clone(),
            password,
        })
    }

    /// Build the full DAV URL for a relative path.
    fn url(&self, remote_path: &str) -> String {
        format!("{}{}", self.dav_base_url, remote_path)
    }

    /// Map HTTP status codes to application errors.
    fn check_status(&self, status: StatusCode, url: &str) -> Result<()> {
        if status.is_success() || status == StatusCode::MULTI_STATUS {
            return Ok(());
        }
        warn!(http_status = %status.as_u16(), %url, "request failed");
        match status.as_u16() {
            401 | 403 => Err(Error::AuthFailed),
            404 => Err(Error::NotFound(url.into())),
            code => Err(Error::WebDav {
                status: code,
                message: status.canonical_reason().unwrap_or("unknown").to_owned(),
            }),
        }
    }
}

impl Backend for NextcloudClient {
    #[tracing::instrument(skip(self), fields(path = %remote_path))]
    async fn list_dir(&self, remote_path: &str) -> Result<Vec<RemoteEntry>> {
        let url = self.url(remote_path);
        // PROPFIND is not a standard HTTP method — build it from bytes.
        // Input is a compile-time constant, so expect() is safe here.
        let method = Method::from_bytes(b"PROPFIND").expect("PROPFIND is a valid HTTP method"); // compile-time constant

        let response = self
            .client
            .request(method, &url)
            .basic_auth(&self.username, Some(self.password.expose_secret()))
            .header("Depth", "1")
            .header(CONTENT_TYPE, "application/xml")
            .body(PROPFIND_BODY)
            .send()
            .await
            .map_err(Error::Http)?;

        let status = response.status();
        self.check_status(status, &url)?;

        let xml = response.text().await.map_err(Error::Http)?;

        let base_path = if remote_path.is_empty() {
            self.dav_base_path.clone()
        } else {
            format!(
                "{}{}",
                self.dav_base_path,
                remote_path.trim_end_matches('/')
            ) + "/"
        };

        let entries = parse_propfind_response(&xml, &base_path)?;
        debug!(count = entries.len(), "listed");
        Ok(entries)
    }

    #[tracing::instrument(skip(self), fields(path = %remote_path))]
    async fn get_metadata(&self, remote_path: &str) -> Result<RemoteEntry> {
        let url = self.url(remote_path);
        let method = Method::from_bytes(b"PROPFIND").expect("PROPFIND is a valid HTTP method"); // compile-time constant

        let response = self
            .client
            .request(method, &url)
            .basic_auth(&self.username, Some(self.password.expose_secret()))
            .header("Depth", "0")
            .header(CONTENT_TYPE, "application/xml")
            .body(PROPFIND_BODY)
            .send()
            .await
            .map_err(Error::Http)?;

        let status = response.status();
        self.check_status(status, &url)?;

        let xml = response.text().await.map_err(Error::Http)?;

        // For Depth:0, the response contains just the requested entry.
        // We use the parent path as base so the entry itself is preserved.
        let parent_path = parent_dav_path(&self.dav_base_path, remote_path);
        let mut entries = parse_propfind_response(&xml, &parent_path)?;

        let entry = entries
            .pop()
            .ok_or_else(|| Error::NotFound(remote_path.into()))?;
        debug!("ok");
        Ok(entry)
    }

    #[tracing::instrument(skip(self), fields(path = %remote_path))]
    async fn download(&self, remote_path: &str) -> Result<Bytes> {
        let url = self.url(remote_path);

        let response = self
            .client
            .get(&url)
            .basic_auth(&self.username, Some(self.password.expose_secret()))
            .send()
            .await
            .map_err(Error::Http)?;

        let status = response.status();
        self.check_status(status, &url)?;

        let bytes = response.bytes().await.map_err(Error::Http)?;
        debug!("ok");
        Ok(bytes)
    }

    #[tracing::instrument(skip(self, data), fields(path = %remote_path))]
    async fn upload(&self, remote_path: &str, data: Bytes) -> Result<RemoteEntry> {
        let url = self.url(remote_path);

        let response = self
            .client
            .put(&url)
            .basic_auth(&self.username, Some(self.password.expose_secret()))
            .body(data)
            .send()
            .await
            .map_err(Error::Http)?;

        let status = response.status();
        self.check_status(status, &url)?;

        // After upload, fetch metadata to get the server-assigned ETag
        let entry = self.get_metadata(remote_path).await?;
        debug!("ok");
        Ok(entry)
    }

    #[tracing::instrument(skip(self), fields(path = %remote_path))]
    async fn delete(&self, remote_path: &str) -> Result<()> {
        let url = self.url(remote_path);

        let response = self
            .client
            .delete(&url)
            .basic_auth(&self.username, Some(self.password.expose_secret()))
            .send()
            .await
            .map_err(Error::Http)?;

        let status = response.status();
        self.check_status(status, &url)?;
        debug!("ok");
        Ok(())
    }

    #[tracing::instrument(skip(self), fields(from = %from, to = %to))]
    async fn move_entry(&self, from: &str, to: &str) -> Result<()> {
        let url = self.url(from);
        let dest_url = self.url(to);
        let method = Method::from_bytes(b"MOVE").expect("MOVE is a valid HTTP method"); // compile-time constant

        let response = self
            .client
            .request(method, &url)
            .basic_auth(&self.username, Some(self.password.expose_secret()))
            .header(
                "Destination",
                HeaderValue::from_str(&dest_url).map_err(|e| Error::WebDav {
                    status: 0,
                    message: format!("invalid destination header: {e}"),
                })?,
            )
            .header("Overwrite", "F")
            .send()
            .await
            .map_err(Error::Http)?;

        let status = response.status();
        self.check_status(status, &url)?;
        debug!("ok");
        Ok(())
    }

    #[tracing::instrument(skip(self), fields(path = %remote_path))]
    async fn create_dir(&self, remote_path: &str) -> Result<()> {
        let url = self.url(remote_path);
        let method = Method::from_bytes(b"MKCOL").expect("MKCOL is a valid HTTP method"); // compile-time constant

        let response = self
            .client
            .request(method, &url)
            .basic_auth(&self.username, Some(self.password.expose_secret()))
            .send()
            .await
            .map_err(Error::Http)?;

        let status = response.status();
        self.check_status(status, &url)?;
        debug!("ok");
        Ok(())
    }

    #[tracing::instrument(skip(self))]
    async fn ping(&self) -> Result<()> {
        let url = self.url("");
        let method = Method::from_bytes(b"PROPFIND").expect("PROPFIND is a valid HTTP method");

        let response = self
            .client
            .request(method, &url)
            .basic_auth(&self.username, Some(self.password.expose_secret()))
            .header("Depth", "0")
            .header(CONTENT_TYPE, "application/xml")
            .body(PROPFIND_BODY)
            .timeout(std::time::Duration::from_secs(3))
            .send()
            .await
            .map_err(Error::Http)?;

        let status = response.status();
        self.check_status(status, &url)?;
        debug!("ok");
        Ok(())
    }
}

/// Compute the parent DAV path for a given remote path.
/// Used to determine the base path for Depth:0 PROPFIND responses.
fn parent_dav_path(dav_base_path: &str, remote_path: &str) -> String {
    let trimmed = remote_path.trim_end_matches('/');
    if let Some(idx) = trimmed.rfind('/') {
        format!("{}{}/", dav_base_path, &trimmed[..idx])
    } else {
        dav_base_path.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_dav_path_file() {
        let base = "/remote.php/dav/files/user/";
        assert_eq!(
            parent_dav_path(base, "Documents/file.txt"),
            "/remote.php/dav/files/user/Documents/"
        );
    }

    #[test]
    fn parent_dav_path_root() {
        let base = "/remote.php/dav/files/user/";
        assert_eq!(
            parent_dav_path(base, "file.txt"),
            "/remote.php/dav/files/user/"
        );
    }

    #[test]
    fn parent_dav_path_dir() {
        let base = "/remote.php/dav/files/user/";
        assert_eq!(
            parent_dav_path(base, "Documents/subdir/"),
            "/remote.php/dav/files/user/Documents/"
        );
    }
}

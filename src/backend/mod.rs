// Backend abstraction layer.
//
// Currently Nextcloud-only. The backend trait will be extracted
// once the Nextcloud implementation stabilizes, enabling
// multi-provider support (Google Drive, OneDrive, S3, etc.).

pub mod nextcloud;

// Nextcloud WebDAV backend.
//
// Implements cloud storage operations via WebDAV protocol:
// - List files (PROPFIND)
// - Download / Upload files (GET / PUT)
// - Delete / Rename (DELETE / MOVE)
// - Change notifications via notify_push

// FUSE filesystem implementation.
//
// Handles all FUSE callbacks (getattr, readdir, open, read, write, etc.)
// by delegating to the local DB for metadata and to background workers
// for file content operations. No network I/O is performed directly
// in FUSE callbacks.

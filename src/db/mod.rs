// SQLite metadata database.
//
// Stores file metadata (name, size, hash, ETag, permissions, timestamps)
// and serves as the authoritative source for readdir() and getattr()
// responses. All metadata queries are answered from this local DB,
// ensuring instant response times and offline capability.

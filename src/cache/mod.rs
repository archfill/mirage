// LRU cache management.
//
// Manages locally cached file data with configurable capacity limits.
// Implements LRU eviction to stay within the configured cache size,
// while respecting pinned files that are excluded from eviction.

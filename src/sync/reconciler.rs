// Reconciliation logic for metadata sync.
//
// Pure function that compares remote and local entries to produce
// a list of actions (Insert, Update, Delete) without any I/O.

use std::collections::HashMap;

use crate::backend::RemoteEntry;
use crate::db::models::{FileEntry, NewFileEntry, SyncState};

/// An action to apply to the local database.
#[derive(Debug, PartialEq)]
pub enum SyncAction {
    Insert(NewFileEntry),
    Update { inode: u64, entry: NewFileEntry },
    Delete { inode: u64 },
}

/// Compare remote and local entries for a single directory,
/// returning a list of actions to synchronize the local DB.
///
/// This is a pure function with no I/O — easy to test.
pub fn reconcile(
    parent_inode: u64,
    remote_entries: &[RemoteEntry],
    local_entries: &[FileEntry],
) -> Vec<SyncAction> {
    let remote_map: HashMap<&str, &RemoteEntry> =
        remote_entries.iter().map(|e| (e.name(), e)).collect();

    let local_map: HashMap<&str, &FileEntry> =
        local_entries.iter().map(|e| (e.name.as_str(), e)).collect();

    let mut actions = Vec::new();

    // Remote-only or changed entries
    for (name, remote) in &remote_map {
        match local_map.get(name) {
            None => {
                // New entry — files start as PendingDownload, dirs as Synced
                let mut new_entry = remote.to_new_file_entry(parent_inode);
                if !remote.is_dir {
                    new_entry.sync_state = SyncState::PendingDownload;
                }
                actions.push(SyncAction::Insert(new_entry));
            }
            Some(local) => {
                if entry_changed(remote, local) {
                    let mut new_entry = remote.to_new_file_entry(parent_inode);
                    // Preserve local-only state
                    new_entry.is_pinned = local.is_pinned;
                    new_entry.is_cached = local.is_cached;
                    // If local has pending upload and remote also changed, mark as Conflict
                    if local.sync_state == SyncState::PendingUpload {
                        new_entry.sync_state = SyncState::Conflict;
                    } else if !remote.is_dir {
                        new_entry.sync_state = SyncState::PendingDownload;
                    }
                    actions.push(SyncAction::Update {
                        inode: local.inode,
                        entry: new_entry,
                    });
                }
            }
        }
    }

    // Local-only entries → delete unless PendingUpload
    for (name, local) in &local_map {
        if !remote_map.contains_key(name)
            && local.sync_state != SyncState::PendingUpload
            && local.sync_state != SyncState::Conflict
        {
            actions.push(SyncAction::Delete { inode: local.inode });
        }
    }

    actions
}

/// Check if a remote entry differs from a local entry.
/// Uses etag if available, otherwise falls back to mtime + size.
fn entry_changed(remote: &RemoteEntry, local: &FileEntry) -> bool {
    match (&remote.etag, &local.etag) {
        (Some(r_etag), Some(l_etag)) => r_etag != l_etag,
        _ => remote.mtime != local.mtime || remote.size != local.size,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_remote(name: &str, is_dir: bool, etag: Option<&str>) -> RemoteEntry {
        RemoteEntry {
            path: name.to_owned(),
            is_dir,
            size: 100,
            mtime: 1_000_000,
            etag: etag.map(|s| s.to_owned()),
            content_hash: None,
            content_type: None,
        }
    }

    fn make_local(
        inode: u64,
        parent: u64,
        name: &str,
        is_dir: bool,
        etag: Option<&str>,
    ) -> FileEntry {
        FileEntry {
            inode,
            parent_inode: parent,
            name: name.to_owned(),
            is_dir,
            size: 100,
            permissions: if is_dir { 0o755 } else { 0o644 },
            mtime: 1_000_000,
            etag: etag.map(|s| s.to_owned()),
            content_hash: None,
            is_pinned: false,
            is_cached: false,
            sync_state: SyncState::Synced,
        }
    }

    // 1. Empty remote + empty local → no actions
    #[test]
    fn empty_both_sides() {
        let actions = reconcile(1, &[], &[]);
        assert!(actions.is_empty());
    }

    // 2. New remote entries → all Insert
    #[test]
    fn new_remote_entries_insert() {
        let remotes = vec![
            make_remote("a.txt", false, Some("e1")),
            make_remote("docs", true, Some("e2")),
        ];
        let actions = reconcile(1, &remotes, &[]);
        assert_eq!(actions.len(), 2);

        let inserts: Vec<_> = actions
            .iter()
            .filter_map(|a| match a {
                SyncAction::Insert(e) => Some(e),
                _ => None,
            })
            .collect();
        assert_eq!(inserts.len(), 2);

        // File should be PendingDownload
        let file_insert = inserts.iter().find(|e| e.name == "a.txt").unwrap();
        assert_eq!(file_insert.sync_state, SyncState::PendingDownload);

        // Directory should be Synced
        let dir_insert = inserts.iter().find(|e| e.name == "docs").unwrap();
        assert_eq!(dir_insert.sync_state, SyncState::Synced);
    }

    // 3. Matching etag → no actions
    #[test]
    fn matching_etag_no_action() {
        let remotes = vec![make_remote("a.txt", false, Some("e1"))];
        let locals = vec![make_local(2, 1, "a.txt", false, Some("e1"))];
        let actions = reconcile(1, &remotes, &locals);
        assert!(actions.is_empty());
    }

    // 4. Changed etag → Update
    #[test]
    fn changed_etag_update() {
        let remotes = vec![make_remote("a.txt", false, Some("e2"))];
        let locals = vec![make_local(2, 1, "a.txt", false, Some("e1"))];
        let actions = reconcile(1, &remotes, &locals);
        assert_eq!(actions.len(), 1);
        assert!(matches!(&actions[0], SyncAction::Update { inode: 2, .. }));
    }

    // 5. Remote-deleted entry → Delete
    #[test]
    fn remote_deleted_entry() {
        let locals = vec![make_local(2, 1, "gone.txt", false, Some("e1"))];
        let actions = reconcile(1, &[], &locals);
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0], SyncAction::Delete { inode: 2 });
    }

    // 6. PendingUpload entry not on remote → no Delete
    #[test]
    fn pending_upload_not_deleted() {
        let mut local = make_local(2, 1, "new.txt", false, Some("e1"));
        local.sync_state = SyncState::PendingUpload;
        let actions = reconcile(1, &[], &[local]);
        assert!(actions.is_empty());
    }

    // 7. Mixed scenario: add + update + delete
    #[test]
    fn mixed_scenario() {
        let remotes = vec![
            make_remote("existing.txt", false, Some("e2")), // changed
            make_remote("new.txt", false, Some("e3")),      // new
        ];
        let locals = vec![
            make_local(2, 1, "existing.txt", false, Some("e1")), // will update
            make_local(3, 1, "removed.txt", false, Some("e4")),  // will delete
        ];
        let actions = reconcile(1, &remotes, &locals);
        assert_eq!(actions.len(), 3);

        let has_insert = actions
            .iter()
            .any(|a| matches!(a, SyncAction::Insert(e) if e.name == "new.txt"));
        let has_update = actions
            .iter()
            .any(|a| matches!(a, SyncAction::Update { inode: 2, .. }));
        let has_delete = actions
            .iter()
            .any(|a| matches!(a, SyncAction::Delete { inode: 3 }));
        assert!(has_insert);
        assert!(has_update);
        assert!(has_delete);
    }

    // 8. Update preserves is_pinned / is_cached
    #[test]
    fn update_preserves_local_flags() {
        let remotes = vec![make_remote("a.txt", false, Some("e2"))];
        let mut local = make_local(2, 1, "a.txt", false, Some("e1"));
        local.is_pinned = true;
        local.is_cached = true;
        let actions = reconcile(1, &remotes, &[local]);

        match &actions[0] {
            SyncAction::Update { entry, .. } => {
                assert!(entry.is_pinned);
                assert!(entry.is_cached);
            }
            _ => panic!("expected Update"),
        }
    }

    // 9. PendingUpload + remote changed → conflict (mark as Conflict)
    #[test]
    fn conflict_pending_upload_remote_changed() {
        let remotes = vec![make_remote("conflict.txt", false, Some("e2"))];
        let mut local = make_local(2, 1, "conflict.txt", false, Some("e1"));
        local.sync_state = SyncState::PendingUpload;
        let actions = reconcile(1, &remotes, &[local]);

        assert_eq!(actions.len(), 1);
        match &actions[0] {
            SyncAction::Update { entry, .. } => {
                assert_eq!(entry.sync_state, SyncState::Conflict);
            }
            _ => panic!("expected Update"),
        }
    }
}

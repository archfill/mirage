// Socket client for communicating with the mirage daemon via Unix domain socket.
// Sends JSON requests and parses JSON responses using the mirage IPC protocol.

#ifndef MIRAGESOCKETCLIENT_H
#define MIRAGESOCKETCLIENT_H

#include <QString>
#include <QLocalSocket>

/// File status as reported by the mirage daemon.
enum class FileStatus {
    Unknown,     // No response or daemon not running
    CloudOnly,   // Not cached locally (blue cloud)
    Cached,      // Downloaded and available locally (green check)
    Syncing,     // Upload or download in progress (spinning arrow)
    Error,       // Conflict or other error state
};

/// Extended file info including pin state.
struct FileInfo {
    FileStatus status = FileStatus::Unknown;
    bool isPinned = false;
    bool isDir = false;
};

/// Synchronous client for the mirage daemon IPC socket.
///
/// Connects to the daemon's Unix domain socket, sends a GetFileStatus
/// request for the given path, and returns the parsed FileStatus.
class MirageSocketClient {
public:
    MirageSocketClient();

    /// Query the daemon for the sync status of the file at @p path.
    /// Returns FileStatus::Unknown if the daemon is unreachable.
    FileStatus queryFileStatus(const QString &path);

    /// Query extended file info (status + pin state).
    FileInfo queryFileInfo(const QString &path);

    /// Request the daemon to pin or unpin a file/directory.
    bool setPinned(const QString &path, bool pinned, bool recursive);

    /// Returns true if the last query succeeded.
    bool isConnected() const { return m_connected; }

private:
    QString socketPath() const;
    QJsonDocument sendRequest(const QJsonObject &request);
    bool m_connected = false;
};

#endif // MIRAGESOCKETCLIENT_H

// KDE Dolphin overlay icon plugin for mirage.
//
// Shows overlay icons on files in the mirage mount to indicate their
// sync status: cloud-only, cached, syncing, or error/conflict.

#ifndef MIRAGEOVERLAYPLUGIN_H
#define MIRAGEOVERLAYPLUGIN_H

#include <KOverlayIconPlugin>
#include <QHash>
#include <QMutex>
#include <QTimer>
#include <QElapsedTimer>

#include "miragesocketclient.h"

/// Dolphin overlay icon plugin that queries the mirage daemon for file status.
///
/// For each file in the mount, getOverlays() returns an icon name based on
/// the file's sync state. Results are cached with a TTL to avoid excessive
/// IPC round-trips on every Dolphin repaint.
class MirageOverlayPlugin : public KOverlayIconPlugin {
    Q_OBJECT
    Q_PLUGIN_METADATA(IID "org.kde.overlayicon/1.0" FILE "mirageoverlayplugin.json")

public:
    explicit MirageOverlayPlugin(QObject *parent = nullptr);

    QStringList getOverlays(const QUrl &item) override;

private Q_SLOTS:
    void onFilesChanged(const QStringList &paths);

private:
    struct CacheEntry {
        FileInfo info;
        QElapsedTimer timer;
    };

    MirageSocketClient m_client;
    QHash<QString, CacheEntry> m_cache;
    QMutex m_mutex;

    static constexpr int CacheTtlMs = 5000;

    QStringList overlaysForInfo(const FileInfo &info) const;
    QString iconForStatus(FileStatus status) const;
};

#endif // MIRAGEOVERLAYPLUGIN_H

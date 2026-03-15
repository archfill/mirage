#include "mirageoverlayplugin.h"

#include <KDirNotify>
#include <QUrl>

MirageOverlayPlugin::MirageOverlayPlugin(QObject *parent)
    : KOverlayIconPlugin(parent)
{
    // Listen for FilesChanged DBus signals to invalidate cache immediately
    auto *watcher = new org::kde::KDirNotify(QString(), QString(), QDBusConnection::sessionBus(), this);
    connect(watcher, &org::kde::KDirNotify::FilesChanged,
            this, &MirageOverlayPlugin::onFilesChanged);
}

void MirageOverlayPlugin::onFilesChanged(const QStringList &urlStrings)
{
    QList<QUrl> urls;
    for (const auto &s : urlStrings) {
        urls << QUrl(s);
    }

    {
        QMutexLocker lock(&m_mutex);
        for (const auto &url : urls) {
            if (url.isLocalFile()) {
                m_cache.remove(url.toLocalFile());
            }
        }
    }
    // Tell Dolphin to re-query overlays for these URLs
    for (const auto &url : urls) {
        Q_EMIT overlaysChanged(url, {});
    }
}

QStringList MirageOverlayPlugin::getOverlays(const QUrl &item)
{
    if (!item.isLocalFile()) {
        return {};
    }

    const QString path = item.toLocalFile();

    // Only process files under a mirage mount (heuristic: check daemon connectivity)
    {
        QMutexLocker lock(&m_mutex);

        // Check cache first
        auto it = m_cache.find(path);
        if (it != m_cache.end() && it->timer.elapsed() < CacheTtlMs) {
            return overlaysForInfo(it->info);
        }
    }

    // Query daemon (outside lock to avoid blocking)
    FileInfo info = m_client.queryFileInfo(path);

    if (info.status == FileStatus::Unknown) {
        // Daemon not reachable or file not in mount — no overlay
        return {};
    }

    {
        QMutexLocker lock(&m_mutex);
        CacheEntry entry;
        entry.info = info;
        entry.timer.start();
        m_cache[path] = entry;
    }

    return overlaysForInfo(info);
}

QStringList MirageOverlayPlugin::overlaysForInfo(const FileInfo &info) const
{
    QStringList icons;

    const QString statusIcon = iconForStatus(info.status);
    if (!statusIcon.isEmpty()) {
        icons << statusIcon;
    }

    if (info.isPinned) {
        icons << QStringLiteral("window-pin");
    }

    return icons;
}

QString MirageOverlayPlugin::iconForStatus(FileStatus status) const
{
    switch (status) {
    case FileStatus::CloudOnly:
        return QStringLiteral("vcs-update-required");
    case FileStatus::Cached:
        return QStringLiteral("vcs-normal");
    case FileStatus::Syncing:
        return QStringLiteral("vcs-locally-modified");
    case FileStatus::Error:
        return QStringLiteral("vcs-conflicting");
    case FileStatus::Unknown:
        return {};
    }
    return {};
}

#include "mirageactionplugin.h"
#include "miragesocketclient.h"

#include <KDirNotify>
#include <KFileItem>
#include <KFileItemListProperties>
#include <QAction>
#include <QMenu>
#include <QUrl>
#include <QWidget>

namespace {

void doPinAction(const QStringList &paths, bool pinned, bool recursive)
{
    MirageSocketClient c;
    QList<QUrl> urls;
    for (const auto &p : paths) {
        c.setPinned(p, pinned, recursive);
        urls << QUrl::fromLocalFile(p);
    }
    // Notify Dolphin to refresh overlays immediately
    org::kde::KDirNotify::emitFilesChanged(urls);
}

} // namespace

MirageActionPlugin::MirageActionPlugin(QObject *parent)
    : KAbstractFileItemActionPlugin(parent)
{
}

QList<QAction *> MirageActionPlugin::actions(const KFileItemListProperties &props,
                                              QWidget *parent)
{
    if (!props.isLocal()) {
        return {};
    }

    const auto items = props.items();
    if (items.isEmpty()) {
        return {};
    }

    // Query file info for the first selected item
    const QString firstPath = items.first().localPath();

    MirageSocketClient client;
    FileInfo info = client.queryFileInfo(firstPath);

    if (info.status == FileStatus::Unknown) {
        return {};
    }

    QStringList paths;
    for (const auto &item : items) {
        paths << item.localPath();
    }

    QList<QAction *> result;

    if (info.isDir) {
        auto *menu = new QMenu(parent);

        if (info.isPinned) {
            menu->setTitle(QStringLiteral("Unpin"));
            menu->setIcon(QIcon::fromTheme(QStringLiteral("window-unpin")));

            auto *thisOnly = menu->addAction(QStringLiteral("This folder only"));
            auto *recursive = menu->addAction(QStringLiteral("Folder and all contents"));

            QObject::connect(thisOnly, &QAction::triggered, parent, [paths]() {
                doPinAction(paths, false, false);
            });
            QObject::connect(recursive, &QAction::triggered, parent, [paths]() {
                doPinAction(paths, false, true);
            });
        } else {
            menu->setTitle(QStringLiteral("Pin"));
            menu->setIcon(QIcon::fromTheme(QStringLiteral("window-pin")));

            auto *thisOnly = menu->addAction(QStringLiteral("This folder only"));
            auto *recursive = menu->addAction(QStringLiteral("Folder and all contents"));

            QObject::connect(thisOnly, &QAction::triggered, parent, [paths]() {
                doPinAction(paths, true, false);
            });
            QObject::connect(recursive, &QAction::triggered, parent, [paths]() {
                doPinAction(paths, true, true);
            });
        }

        result << menu->menuAction();
    } else {
        if (info.isPinned) {
            auto *action = new QAction(QIcon::fromTheme(QStringLiteral("window-unpin")),
                                       QStringLiteral("Unpin"),
                                       parent);
            QObject::connect(action, &QAction::triggered, parent, [paths]() {
                doPinAction(paths, false, false);
            });
            result << action;
        } else {
            auto *action = new QAction(QIcon::fromTheme(QStringLiteral("window-pin")),
                                       QStringLiteral("Pin (keep local)"),
                                       parent);
            QObject::connect(action, &QAction::triggered, parent, [paths]() {
                doPinAction(paths, true, false);
            });
            result << action;
        }
    }

    return result;
}

K_PLUGIN_CLASS_WITH_JSON(MirageActionPlugin, "mirageactionplugin.json")

#include "mirageactionplugin.moc"

#ifndef MIRAGEACTIONPLUGIN_H
#define MIRAGEACTIONPLUGIN_H

#include <KAbstractFileItemActionPlugin>
#include <KPluginFactory>

class MirageActionPlugin : public KAbstractFileItemActionPlugin {
    Q_OBJECT

public:
    explicit MirageActionPlugin(QObject *parent = nullptr);

    QList<QAction *> actions(const KFileItemListProperties &props,
                             QWidget *parent) override;
};

#endif // MIRAGEACTIONPLUGIN_H

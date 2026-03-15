#include "miragesocketclient.h"

#include <QDir>
#include <QJsonDocument>
#include <QJsonObject>
#include <QLocalSocket>
#include <QStandardPaths>

MirageSocketClient::MirageSocketClient() = default;

QString MirageSocketClient::socketPath() const
{
    // Match the Rust daemon: $XDG_RUNTIME_DIR/mirage.sock or /tmp/mirage.sock
    const QString runtimeDir = QStandardPaths::writableLocation(QStandardPaths::RuntimeLocation);
    if (!runtimeDir.isEmpty()) {
        return runtimeDir + QStringLiteral("/mirage.sock");
    }
    return QStringLiteral("/tmp/mirage.sock");
}

QJsonDocument MirageSocketClient::sendRequest(const QJsonObject &request)
{
    m_connected = false;

    QLocalSocket socket;
    socket.connectToServer(socketPath());
    if (!socket.waitForConnected(100)) {
        return {};
    }

    QByteArray data = QJsonDocument(request).toJson(QJsonDocument::Compact);
    data.append('\n');
    socket.write(data);
    if (!socket.waitForBytesWritten(200)) {
        return {};
    }

    if (!socket.waitForReadyRead(200)) {
        return {};
    }

    const QByteArray response = socket.readLine();
    socket.disconnectFromServer();

    m_connected = true;

    return QJsonDocument::fromJson(response);
}

FileStatus MirageSocketClient::queryFileStatus(const QString &path)
{
    QJsonObject inner;
    inner[QStringLiteral("path")] = path;
    QJsonObject request;
    request[QStringLiteral("GetFileStatus")] = inner;

    const QJsonDocument doc = sendRequest(request);
    if (doc.isNull() || !doc.isObject()) {
        return FileStatus::Unknown;
    }

    const QJsonObject obj = doc.object();

    // Response format: {"FileStatus":"Cached"} or {"Error":"..."}
    if (obj.contains(QStringLiteral("FileStatus"))) {
        const QString status = obj[QStringLiteral("FileStatus")].toString();
        if (status == QStringLiteral("CloudOnly")) return FileStatus::CloudOnly;
        if (status == QStringLiteral("Cached"))    return FileStatus::Cached;
        if (status == QStringLiteral("Syncing"))   return FileStatus::Syncing;
        if (status == QStringLiteral("Error"))     return FileStatus::Error;
    }

    return FileStatus::Unknown;
}

FileInfo MirageSocketClient::queryFileInfo(const QString &path)
{
    QJsonObject inner;
    inner[QStringLiteral("path")] = path;
    QJsonObject request;
    request[QStringLiteral("GetFileStatus")] = inner;

    const QJsonDocument doc = sendRequest(request);
    if (doc.isNull() || !doc.isObject()) {
        return {};
    }

    const QJsonObject obj = doc.object();

    // Response format: {"FileInfo":{"status":"Cached","is_pinned":true,"is_dir":false}}
    if (!obj.contains(QStringLiteral("FileInfo"))) {
        return {};
    }

    const QJsonObject fileInfoObj = obj[QStringLiteral("FileInfo")].toObject();

    FileInfo info;
    info.isPinned = fileInfoObj[QStringLiteral("is_pinned")].toBool(false);
    info.isDir    = fileInfoObj[QStringLiteral("is_dir")].toBool(false);

    const QString statusStr = fileInfoObj[QStringLiteral("status")].toString();
    if (statusStr == QStringLiteral("CloudOnly"))      info.status = FileStatus::CloudOnly;
    else if (statusStr == QStringLiteral("Cached"))    info.status = FileStatus::Cached;
    else if (statusStr == QStringLiteral("Syncing"))   info.status = FileStatus::Syncing;
    else if (statusStr == QStringLiteral("Error"))     info.status = FileStatus::Error;
    else                                               info.status = FileStatus::Unknown;

    return info;
}

bool MirageSocketClient::setPinned(const QString &path, bool pinned, bool recursive)
{
    QJsonObject inner;
    inner[QStringLiteral("path")]      = path;
    inner[QStringLiteral("pinned")]    = pinned;
    inner[QStringLiteral("recursive")] = recursive;
    QJsonObject request;
    request[QStringLiteral("SetPinned")] = inner;

    const QJsonDocument doc = sendRequest(request);
    if (doc.isNull() || !doc.isObject()) {
        return false;
    }

    // Expects: "Ok"
    const QJsonObject obj = doc.object();
    return obj.contains(QStringLiteral("Ok"));
}

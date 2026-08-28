// SPDX-License-Identifier: MIT
// Cua-owned KWin effect. The Rust driver verifies that this D-Bus name is
// owned by the running kwin_wayland process before it trusts any response.
// Protocol v1 is intentionally read-only: only version and window snapshots
// are exported; input and activation remain outside this helper.

#include <kwin/effect/effect.h>
#include <kwin/window.h>
#include <kwin/workspace.h>

#include <QDBusConnection>
#include <QHash>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QSet>
#include <QtMath>
#include <QUuid>

namespace
{
constexpr auto kService = "org.cua.KWinTarget";
constexpr auto kPath = "/org/cua/KWinTarget";
constexpr quint32 kProtocolVersion = 1;

class TargetApi : public QObject
{
    Q_OBJECT
    Q_CLASSINFO("D-Bus Interface", "org.cua.KWinTarget")

public slots:
    Q_SCRIPTABLE quint32 GetVersion()
    {
        return kProtocolVersion;
    }

    Q_SCRIPTABLE QString GetWindows()
    {
        const auto liveWindows = KWin::Workspace::self()->windows();
        QSet<QUuid> liveIds;
        for (KWin::Window *window : liveWindows) {
            if (window && window->isClient() && !window->isDeleted()) {
                liveIds.insert(window->internalId());
            }
        }
        for (auto it = m_tokens.begin(); it != m_tokens.end();) {
            if (!liveIds.contains(it.key())) {
                it = m_tokens.erase(it);
            } else {
                ++it;
            }
        }

        QJsonArray windows;
        for (KWin::Window *window : liveWindows) {
            if (!window || !window->isClient() || window->isDeleted()) {
                continue;
            }
            const auto frame = window->frameGeometry();
            QJsonObject record;
            record["token"] = static_cast<qint64>(tokenFor(window->internalId()));
            record["pid"] = window->pid();
            record["title"] = window->captionNormal();
            record["app_id"] = window->resourceClass();
            record["x"] = qRound(frame.x());
            record["y"] = qRound(frame.y());
            record["w"] = qRound(frame.width());
            record["h"] = qRound(frame.height());
            record["active"] = window->isActive();
            record["minimized"] = window->isMinimized();
            record["stacking"] = window->stackingOrder();
            windows.push_back(record);
        }
        return QString::fromUtf8(QJsonDocument(windows).toJson(QJsonDocument::Compact));
    }

private:
    quint64 tokenFor(const QUuid &id)
    {
        if (const auto token = m_tokens.constFind(id); token != m_tokens.cend()) {
            return *token;
        }
        const quint64 token = m_nextToken++;
        m_tokens.insert(id, token);
        return token;
    }

    QHash<QUuid, quint64> m_tokens;
    quint64 m_nextToken = 1;
};

class KwinTargetHelperEffect : public KWin::Effect
{
public:
    KwinTargetHelperEffect()
    {
        auto bus = QDBusConnection::sessionBus();
        if (!bus.registerService(QString::fromLatin1(kService))) {
            return;
        }
        m_exported = bus.registerObject(
            QString::fromLatin1(kPath),
            &m_api,
            QDBusConnection::ExportScriptableSlots);
        if (!m_exported) {
            bus.unregisterService(QString::fromLatin1(kService));
        }
    }

    ~KwinTargetHelperEffect() override
    {
        if (!m_exported) {
            return;
        }
        auto bus = QDBusConnection::sessionBus();
        bus.unregisterObject(QString::fromLatin1(kPath));
        bus.unregisterService(QString::fromLatin1(kService));
    }

private:
    TargetApi m_api;
    bool m_exported = false;
};
}

KWIN_EFFECT_FACTORY(KwinTargetHelperEffect, "kwin_target_helper.json")

#include "kwin_target_helper.moc"

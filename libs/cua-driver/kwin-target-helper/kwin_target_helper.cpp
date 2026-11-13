// SPDX-License-Identifier: MIT
// Cua-owned KWin effect. The Rust driver verifies that this D-Bus name is
// owned by the running kwin_wayland process before it trusts any response.
//
// Protocol v1 is read-only: version and window snapshots.
//
// Protocol v2 adds an input gate. It never creates, injects or changes input:
// it can only stop Cua Driver's own libei events from reaching a window other
// than the one the driver names. Portal/libei input is focus-bound, so a focus
// check in the driver followed by input always leaves a window in which focus
// can move. The gate closes it: KWin runs this filter on its main thread for
// each event, after it has resolved the event's focus and before any window
// receives the event, so the check and the delivery cannot be separated.

#include <kwin/core/inputdevice.h>
#include <kwin/effect/effect.h>
#include <kwin/input.h>
#include <kwin/input_event.h>
#include <kwin/pointer_input.h>
#include <kwin/wayland/seat.h>
#include <kwin/wayland_server.h>
#include <kwin/window.h>
#include <kwin/workspace.h>

#include <QDBusConnection>
#include <QDBusContext>
#include <QDBusMessage>
#include <QDBusServiceWatcher>
#include <QHash>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QSet>
#include <QtMath>
#include <QUuid>

#include <algorithm>
#include <memory>

namespace
{
constexpr auto kService = "org.cua.KWinTarget";
constexpr auto kPath = "/org/cua/KWinTarget";
constexpr quint32 kProtocolVersion = 2;

// Only devices of Cua Driver's own libei clients can be gated, so no caller
// can use the gate to hold back another program's input.
constexpr auto kClientPrefix = "cua-driver";

QString toJson(const QJsonObject &object)
{
    return QString::fromUtf8(QJsonDocument(object).toJson(QJsonDocument::Compact));
}

// `window` is `target` or one of its transient children (a menu, popup or
// dialog of the target), which is where the target's own input goes.
bool belongsTo(const KWin::Window *window, const KWin::Window *target)
{
    for (int depth = 0; window && depth < 32; ++depth) {
        if (window == target) {
            return true;
        }
        window = window->transientFor();
    }
    return false;
}

KWin::Window *keyboardFocus()
{
    auto server = KWin::waylandServer();
    if (!server || !server->seat()) {
        return nullptr;
    }
    const auto surface = server->seat()->focusedKeyboardSurface();
    if (!surface) {
        return nullptr;
    }
    if (auto window = server->findWindow(surface)) {
        return window;
    }
    // WaylandServer knows only native Wayland windows. An Xwayland window
    // (Java, Wine and other X11-only apps) is in the workspace, with its
    // Xwayland surface.
    auto workspace = KWin::workspace();
    return workspace ? workspace->findWindow([surface](const KWin::Window *window) {
        return window->surface() == surface;
    })
                     : nullptr;
}

KWin::Window *pointerFocus()
{
    auto input = KWin::input();
    return input && input->pointer() ? input->pointer()->focus() : nullptr;
}

struct Arm
{
    quint64 id = 0;
    QUuid target;
    QString client;
    quint64 passed = 0;
    quint64 dropped = 0;
    // Presses that reached the target: their releases always follow them, so
    // no key or button stays held down in the target.
    QSet<quint32> passedKeys;
    QSet<quint32> passedButtons;
    // Presses held back: their releases are held back too, so no window gets
    // a release it never saw pressed.
    QSet<quint32> droppedKeys;
    QSet<quint32> droppedButtons;
};

class InputGate : public KWin::InputEventFilter
{
public:
    InputGate()
        : KWin::InputEventFilter(KWin::InputFilterOrder::PlaceholderOutput)
    {
    }

    std::unique_ptr<Arm> arm;

    bool pointerMotion(KWin::PointerMotionEvent *event) override
    {
        // KWin has already moved the pointer: the gate decides only whether
        // a window other than the target receives the motion.
        return gate(event->device, [&](const KWin::Window *target) {
            return belongsTo(pointerFocus(), target);
        });
    }

    bool pointerButton(KWin::PointerButtonEvent *event) override
    {
        return gateTransition(
            event->device,
            event->state == KWin::PointerButtonState::Pressed,
            event->nativeButton,
            &Arm::passedButtons,
            &Arm::droppedButtons,
            [](const KWin::Window *target) {
                return belongsTo(pointerFocus(), target);
            });
    }

    bool pointerAxis(KWin::PointerAxisEvent *event) override
    {
        return gate(event->device, [](const KWin::Window *target) {
            return belongsTo(pointerFocus(), target);
        });
    }

    bool keyboardKey(KWin::KeyboardKeyEvent *event) override
    {
        if (event->state == KWin::KeyboardKeyState::Repeated) {
            return gate(event->device, [](const KWin::Window *target) {
                return belongsTo(keyboardFocus(), target);
            });
        }
        return gateTransition(
            event->device,
            event->state == KWin::KeyboardKeyState::Pressed,
            event->nativeScanCode,
            &Arm::passedKeys,
            &Arm::droppedKeys,
            [](const KWin::Window *target) {
                return belongsTo(keyboardFocus(), target);
            });
    }

private:
    bool gated(KWin::InputDevice *device) const
    {
        return arm && device && device->name().startsWith(arm->client + QLatin1Char(' '));
    }

    KWin::Window *target() const
    {
        auto workspace = KWin::workspace();
        return workspace ? workspace->findWindow(arm->target) : nullptr;
    }

    // `true` stops the event.
    template<typename Check>
    bool gate(KWin::InputDevice *device, Check reachesTarget)
    {
        if (!gated(device)) {
            return false;
        }
        const auto window = target();
        if (window && !window->isDeleted() && reachesTarget(window)) {
            ++arm->passed;
            return false;
        }
        ++arm->dropped;
        return true;
    }

    template<typename Check>
    bool gateTransition(KWin::InputDevice *device,
                        bool pressed,
                        quint32 code,
                        QSet<quint32> Arm::*passedSet,
                        QSet<quint32> Arm::*droppedSet,
                        Check reachesTarget)
    {
        if (!gated(device)) {
            return false;
        }
        if (!pressed) {
            if ((arm.get()->*droppedSet).remove(code)) {
                return true;
            }
            // A release of a press that passed, or of one made before the
            // gate was armed, always passes.
            (arm.get()->*passedSet).remove(code);
            return false;
        }
        const bool stop = gate(device, reachesTarget);
        ((arm.get())->*(stop ? droppedSet : passedSet)).insert(code);
        return stop;
    }
};

class TargetApi : public QObject, protected QDBusContext
{
    Q_OBJECT
    Q_CLASSINFO("D-Bus Interface", "org.cua.KWinTarget")

public:
    InputGate gate;

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
            record["uuid"] = window->internalId().toString(QUuid::WithoutBraces);
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

    // Where keyboard and pointer input would go right now, relative to the
    // window with KWin internal id `window`.
    Q_SCRIPTABLE QString GetFocus(const QString &window)
    {
        const auto target = windowFor(window);
        QJsonObject state;
        state["known"] = target != nullptr;
        state["keyboard"] = target && belongsTo(keyboardFocus(), target);
        state["pointer"] = target && belongsTo(pointerFocus(), target);
        state["active"] = target && target->isActive();
        return toJson(state);
    }

    // Gate the libei devices of `client` (the name its libei handshake gave,
    // which must start with "cua-driver") to the window with KWin internal id
    // `window`. Only one gate is armed at a time. The gate lasts until the
    // caller disarms it or its D-Bus connection closes, so a driver that dies
    // mid-action leaves nothing armed.
    Q_SCRIPTABLE QString ArmInput(const QString &window, const QString &client)
    {
        QJsonObject reply;
        if (!client.startsWith(QLatin1String(kClientPrefix))) {
            reply["error"] = QStringLiteral("only a cua-driver libei client can be gated");
            return toJson(reply);
        }
        const auto target = windowFor(window);
        if (!target) {
            reply["error"] = QStringLiteral("no live window has this id");
            return toJson(reply);
        }
        if (gate.arm) {
            reply["error"] = QStringLiteral("another input gate is armed");
            return toJson(reply);
        }
        // A gate over no device would hold nothing back while the caller
        // believes its input is bound to the target.
        if (!hasDevice(client)) {
            reply["error"] = QStringLiteral("no libei device of this client is connected");
            return toJson(reply);
        }
        if (!calledFromDBus()) {
            reply["error"] = QStringLiteral("the input gate is armed only over D-Bus");
            return toJson(reply);
        }
        auto arm = std::make_unique<Arm>();
        arm->id = m_nextArm++;
        arm->target = target->internalId();
        arm->client = client;
        const quint64 id = arm->id;
        gate.arm = std::move(arm);
        m_owner = std::make_unique<QDBusServiceWatcher>(message().service(),
                                                        connection(),
                                                        QDBusServiceWatcher::WatchForUnregistration);
        connect(m_owner.get(), &QDBusServiceWatcher::serviceUnregistered, this, [this, id] {
            if (gate.arm && gate.arm->id == id) {
                gate.arm.reset();
            }
            dropOwner();
        });
        reply["arm"] = static_cast<qint64>(id);
        return toJson(reply);
    }

    Q_SCRIPTABLE QString GetInputGate(quint64 arm)
    {
        return armState(arm, false);
    }

    Q_SCRIPTABLE QString DisarmInput(quint64 arm)
    {
        return armState(arm, true);
    }

private:
    QString armState(quint64 id, bool disarm)
    {
        QJsonObject reply;
        if (!gate.arm || gate.arm->id != id) {
            reply["error"] = QStringLiteral("this input gate is not armed");
            return toJson(reply);
        }
        reply["passed"] = static_cast<qint64>(gate.arm->passed);
        reply["dropped"] = static_cast<qint64>(gate.arm->dropped);
        if (disarm) {
            gate.arm.reset();
            dropOwner();
        }
        return toJson(reply);
    }

    // The watcher may be the sender of the signal being handled.
    void dropOwner()
    {
        if (m_owner) {
            m_owner.release()->deleteLater();
        }
    }

    static bool hasDevice(const QString &client)
    {
        const auto prefix = client + QLatin1Char(' ');
        const auto devices = KWin::input()->devices();
        return std::any_of(devices.cbegin(), devices.cend(), [&](const KWin::InputDevice *device) {
            return device->name().startsWith(prefix);
        });
    }

    KWin::Window *windowFor(const QString &id)
    {
        const QUuid uuid = QUuid::fromString(id);
        if (uuid.isNull()) {
            return nullptr;
        }
        auto window = KWin::workspace()->findWindow(uuid);
        return window && window->isClient() && !window->isDeleted() ? window : nullptr;
    }

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
    quint64 m_nextArm = 1;
    // Watches the D-Bus connection that armed the gate.
    std::unique_ptr<QDBusServiceWatcher> m_owner;
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
            return;
        }
        KWin::input()->installInputEventFilter(&m_api.gate);
    }

    ~KwinTargetHelperEffect() override
    {
        if (!m_exported) {
            return;
        }
        KWin::input()->uninstallInputEventFilter(&m_api.gate);
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

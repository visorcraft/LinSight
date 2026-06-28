// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only
//
// In-app screenshot helper. The Wayland compositor returns the last
// cached surface for unfocused windows, so external screenshot tools
// (spectacle, grim) can't reliably capture LinSight from a headless
// dev script. QQuickWindow::grabWindow() forces a fresh render of the
// QML scene into an offscreen image regardless of focus / visibility
// — exactly what we want for screenshot-driven UI iteration.
//
// Implemented with plain QTimer::singleShot lambdas so the file
// doesn't need MOC processing.

#include <QCoreApplication>
#include <QGuiApplication>
#include <QImage>
#include <QQuickWindow>
#include <QString>
#include <QTimer>
#include <QWindow>
#include <QtDebug>

namespace {

// Time between window-not-yet-visible retries. Multiplied by
// `kMaxGrabRetries` to get the overall retry budget — bump together if
// a cold-cache QML scene is slower to come up on a particular host.
constexpr int kRetryIntervalMs = 100;
// Cap on the retry count. After this many `kRetryIntervalMs` polls
// without seeing a visible QQuickWindow, we abandon and exit(2). The
// product (retries × interval) bounds how long the binary will hang
// looking for a window before reporting failure.
constexpr int kMaxGrabRetries = 20;
// Tiny pause after a successful save before exiting the event loop, so
// QImage::save's underlying file-close + fsync has time to complete
// before the process tears down. Empirically necessary on some
// filesystems; 50ms is invisible to humans and longer than any
// observed fsync window during testing.
constexpr int kPostSaveSettleMs = 50;

QQuickWindow *findVisibleWindow() {
    auto *gui = qobject_cast<QGuiApplication *>(QCoreApplication::instance());
    if (gui == nullptr) {
        return nullptr;
    }
    const auto windows = gui->topLevelWindows();
    for (QWindow *w : windows) {
        if (auto *qw = qobject_cast<QQuickWindow *>(w)) {
            if (qw->isVisible()) {
                return qw;
            }
        }
    }
    return nullptr;
}

void tryGrab(const QString path, int retries);

void scheduleRetry(const QString path, int retries) {
    QTimer::singleShot(kRetryIntervalMs, QCoreApplication::instance(),
                       [path, retries] { tryGrab(path, retries); });
}

void tryGrab(const QString path, int retries) {
    QQuickWindow *win = findVisibleWindow();
    if (win == nullptr) {
        if (retries > kMaxGrabRetries) {
            qWarning() << "linsight: screenshot abandoned — no QQuickWindow after"
                       << (retries * kRetryIntervalMs) << "ms";
            QCoreApplication::exit(2);
            return;
        }
        scheduleRetry(path, retries + 1);
        return;
    }
    QImage img = win->grabWindow();
    if (img.isNull()) {
        qWarning() << "linsight: grabWindow returned a null image";
        QCoreApplication::exit(3);
        return;
    }
    if (!img.save(path, "PNG")) {
        qWarning() << "linsight: failed to write screenshot to" << path;
        QCoreApplication::exit(4);
        return;
    }
    qInfo() << "linsight: screenshot written to" << path
            << "(" << img.width() << "x" << img.height() << ")";
    QTimer::singleShot(kPostSaveSettleMs, QCoreApplication::instance(),
                       [] { QCoreApplication::exit(0); });
}

} // namespace

extern "C" void linsight_arm_screenshot(const char *path_utf8, int delay_ms) {
    if (path_utf8 == nullptr || *path_utf8 == '\0') {
        return;
    }
    if (QCoreApplication::instance() == nullptr) {
        qWarning() << "linsight: linsight_arm_screenshot called before QCoreApplication exists";
        return;
    }
    const QString path = QString::fromUtf8(path_utf8);
    QTimer::singleShot(delay_ms, QCoreApplication::instance(),
                       [path] { tryGrab(path, 0); });
}

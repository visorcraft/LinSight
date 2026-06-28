// SPDX-FileCopyrightText: 2026 VisorCraft LLC
// SPDX-License-Identifier: GPL-3.0-only
//
// Loads the .qm catalog for the system locale (bundled at
// qrc:/qt/qml/com/visorcraft/LinSight/i18n/linsight_<lang>.qm) and
// installs it on the QCoreApplication. cxx-qt-lib 0.8 doesn't bind
// QTranslator yet, so we expose this as a plain extern-"C" entrypoint
// and call it from Rust via FFI.

#include <QCoreApplication>
#include <QLocale>
#include <QString>
#include <QTranslator>

namespace {
QTranslator *g_translator = nullptr;
}

extern "C" int linsight_install_system_translator() {
    if (g_translator == nullptr) {
        g_translator = new QTranslator(QCoreApplication::instance());
    }
    const QLocale locale = QLocale::system();
    const bool loaded = g_translator->load(
        locale,
        QStringLiteral("linsight"),
        QStringLiteral("_"),
        QStringLiteral(":/qt/qml/com/visorcraft/LinSight/i18n"),
        QStringLiteral(".qm"));
    if (loaded) {
        QCoreApplication::installTranslator(g_translator);
        return 1;
    }
    return 0;
}

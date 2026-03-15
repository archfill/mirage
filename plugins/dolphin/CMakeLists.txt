cmake_minimum_required(VERSION 3.20)
project(mirage-dolphin-plugin VERSION 0.1.0 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_AUTOMOC ON)

set(QT_MAJOR_VERSION 6)

find_package(ECM REQUIRED NO_MODULE)
set(CMAKE_MODULE_PATH ${ECM_MODULE_PATH})

include(KDEInstallDirs)
include(KDECMakeSettings)

find_package(Qt6 REQUIRED COMPONENTS Core DBus Network)
find_package(KF6 REQUIRED COMPONENTS KIO)

add_library(mirageoverlayplugin MODULE
    mirageoverlayplugin.cpp
    miragesocketclient.cpp
)

target_link_libraries(mirageoverlayplugin
    Qt6::Core
    Qt6::DBus
    Qt6::Network
    KF6::KIOCore
    KF6::KIOWidgets
)

install(TARGETS mirageoverlayplugin
    DESTINATION ${KDE_INSTALL_QTPLUGINDIR}/kf6/overlayicon
)

add_library(mirageactionplugin MODULE
    mirageactionplugin.cpp
    miragesocketclient.cpp
)

target_link_libraries(mirageactionplugin
    Qt6::Core
    Qt6::DBus
    Qt6::Network
    KF6::KIOCore
    KF6::KIOWidgets
)

install(TARGETS mirageactionplugin
    DESTINATION ${KDE_INSTALL_QTPLUGINDIR}/kf6/kfileitemaction
)

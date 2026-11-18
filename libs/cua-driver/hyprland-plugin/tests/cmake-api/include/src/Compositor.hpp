#pragma once

#if __cplusplus <= 202302L
#error The window API probe must compile in C++26 mode
#endif
#ifndef CUA_PROBE_FLAG
#error The window API probe must propagate pkg-config flags
#endif

#include <memory>

#if defined(CUA_PROBE_window_backend)
struct IWindowBackend {
    bool isX11() const;
    int pid() const;
};
struct CWindow {
    bool mapped() const;
    const IWindowBackend& backend() const;
};
#elif defined(CUA_PROBE_window_legacy)
struct CWindow {
    bool m_isMapped;
    bool m_isX11;
    int getPID();
};
#else
struct CWindow {};
#endif
using PHLWINDOW = std::shared_ptr<CWindow>;

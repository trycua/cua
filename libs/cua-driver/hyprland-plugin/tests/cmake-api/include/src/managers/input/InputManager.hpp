#pragma once

#include <memory>
#include <vector>

struct CInputManager {
#if defined(CUA_PROBE_window_backend)
    std::vector<int> m_exclusiveKeyboardLSes;
#elif defined(CUA_PROBE_window_legacy)
    std::vector<int> m_exclusiveLSes;
#endif
};
extern std::unique_ptr<CInputManager> g_pInputManager;

#pragma once

#if __cplusplus <= 202302L
#error The plugin API probe must compile in C++26 mode
#endif
#ifndef CUA_PROBE_FLAG
#error The plugin API probe must propagate pkg-config flags
#endif

#if defined(CUA_PROBE_socket1)
namespace IPC::Socket1 {
struct SCommand {};
}
#elif defined(CUA_PROBE_legacy)
struct SHyprCtlCommand {};
#endif

/*
 * Aggregated header for bindgen.
 *
 * toupcam.h is a C-compatible header: its `extern "C"` block and all C++
 * convenience overloads are guarded by `__cplusplus`, it uses no bare `bool`,
 * and on non-Windows it falls back `HRESULT -> int` / `__stdcall -> empty`
 * itself. So — unlike the ZWO EFW/EAF headers — it is parsed as plain C (see
 * build.rs: no `-x c++`). The `#include <windows.h>` inside toupcam.h is guarded
 * by `_WIN32`, so it is only pulled in on Windows targets.
 *
 * TOUPCAM_HRESULT_ERRORCODE_NEEDED is defined in build.rs so the S_OK / E_*
 * HRESULT error-code macros are present in the translation unit for reference
 * (the safe wrapper maps them in error.rs).
 */
#include "toupcam.h"

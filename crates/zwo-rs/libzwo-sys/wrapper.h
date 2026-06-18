/*
 * Aggregated header for bindgen.
 *
 * Parsed as C++ (see build.rs: clang args `-x c++`) so that the bare `bool`
 * used by EFW_filter.h / EAF_focuser.h — which do not #include <stdbool.h> —
 * resolves to the builtin type. The headers are self-contained (no transitive
 * includes), so no additional include paths are required beyond sdk/include.
 */
#include "ASICamera2.h"
#include "EFW_filter.h"
#include "EAF_focuser.h"

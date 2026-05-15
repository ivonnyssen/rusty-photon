#pragma once

// Bridge header that pulls cdc_acm_host.h into the esp-idf-sys bindgen run.
// Without this, the bindings.rs in esp-idf-sys doesn't carry the symbols and
// the link step is left with unresolved references.
#include "usb/cdc_acm_host.h"

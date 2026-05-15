# Local ESP-IDF component whose only purpose is to pull in the managed
# `espressif/usb_host_cdc_acm` component (see idf_component.yml next to this
# file) and re-export its public include path / library so the Rust binary
# can link against it.
idf_component_register(REQUIRES usb_host_cdc_acm)

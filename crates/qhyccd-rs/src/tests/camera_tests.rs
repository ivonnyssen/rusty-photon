use std::ffi::c_char;

use crate::mocks::mock_libqhyccd_sys::{
    BeginQHYCCDLive_context, CancelQHYCCDExposingAndReadout_context, CancelQHYCCDExposing_context,
    CloseQHYCCD_context, ExpQHYCCDSingleFrame_context, GetQHYCCDChipInfo_context,
    GetQHYCCDEffectiveArea_context, GetQHYCCDExposureRemaining_context, GetQHYCCDFWVersion_context,
    GetQHYCCDLiveFrame_context, GetQHYCCDMemLength_context, GetQHYCCDModel_context,
    GetQHYCCDNumberOfReadModes_context, GetQHYCCDOverScanArea_context,
    GetQHYCCDParamMinMaxStep_context, GetQHYCCDParam_context, GetQHYCCDReadModeName_context,
    GetQHYCCDReadModeResolution_context, GetQHYCCDReadMode_context, GetQHYCCDSingleFrame_context,
    GetQHYCCDType_context, InitQHYCCD_context, IsQHYCCDControlAvailable_context,
    OpenQHYCCD_context, SetQHYCCDBinMode_context, SetQHYCCDBitsMode_context,
    SetQHYCCDDebayerOnOff_context, SetQHYCCDParam_context, SetQHYCCDReadMode_context,
    SetQHYCCDResolution_context, SetQHYCCDStreamMode_context, StopQHYCCDLive_context, QHYCCD_ERROR,
    QHYCCD_ERROR_F64, QHYCCD_SUCCESS,
};
use crate::*;

const TEST_HANDLE: *const std::ffi::c_void = 0xdeadbeef as *const std::ffi::c_void;

/// An opened test [`Camera`] bundled with a permissive `CloseQHYCCD` mock
/// expectation. `Camera::new` builds a Real backend whose new `HandleCell::Drop`
/// closes the still-open handle when the camera is dropped at the end of a test;
/// under `#[cfg(test)]` that calls the mocked `CloseQHYCCD`, so an expectation
/// must be live at drop time. Field order is load-bearing: `camera` is declared
/// before `_close_ctx`, so it drops FIRST (firing `CloseQHYCCD`) while the
/// context guard is still alive. Derefs to [`Camera`] so tests call camera
/// methods unchanged.
struct OpenTestCamera<G> {
    camera: Camera,
    _close_ctx: G,
}

impl<G> std::ops::Deref for OpenTestCamera<G> {
    type Target = Camera;
    fn deref(&self) -> &Camera {
        &self.camera
    }
}

fn new_camera() -> OpenTestCamera<impl Sized> {
    let ctx_open = OpenQHYCCD_context();
    ctx_open.expect().times(1).return_const_st(TEST_HANDLE);
    let camera = Camera::new("test_camera".to_owned());
    camera.open().unwrap();
    // `ctx_open` drops here — its `times(1)` is already satisfied by `open()`.
    // The `CloseQHYCCD` expectation must outlive the returned camera, so it is
    // carried out inside the wrapper (dropped after `camera`, see the struct doc).
    let close_ctx = CloseQHYCCD_context();
    close_ctx.expect().return_const_st(QHYCCD_SUCCESS);
    OpenTestCamera {
        camera,
        _close_ctx: close_ctx,
    }
}

#[test]
fn set_stream_mode_success() {
    let _mock = super::mock_guard();
    //given
    let ctx = SetQHYCCDStreamMode_context();
    ctx.expect()
        .withf_st(|_, mode| *mode == StreamMode::LiveMode as u8)
        .times(1)
        .return_const_st(QHYCCD_SUCCESS);
    let cam = new_camera();
    //when
    let res = cam.set_stream_mode(StreamMode::LiveMode);
    //then
    assert!(res.is_ok());
}

#[test]
fn set_stream_mode_fail() {
    let _mock = super::mock_guard();
    //given
    let ctx = SetQHYCCDStreamMode_context();
    ctx.expect().times(1).return_const_st(QHYCCD_ERROR);
    let cam = new_camera();
    //when
    let res = cam.set_stream_mode(StreamMode::LiveMode);
    //then
    assert!(res.is_err());
    assert_eq!(
        res.err().unwrap(),
        QHYError::Sdk {
            op: "set_stream_mode"
        }
    );
}

#[test]
fn set_readout_mode_success() {
    let _mock = super::mock_guard();
    //given
    let ctx = SetQHYCCDReadMode_context();
    ctx.expect()
        .withf_st(|_, mode| *mode == 1_u32)
        .times(1)
        .return_const_st(QHYCCD_SUCCESS);
    let cam = new_camera();
    //when
    let res = cam.set_readout_mode(1_u32);
    //then
    assert!(res.is_ok());
}

#[test]
fn set_readout_mode_fail() {
    let _mock = super::mock_guard();
    //given
    let ctx = SetQHYCCDReadMode_context();
    ctx.expect().times(1).return_const_st(QHYCCD_ERROR);
    let cam = new_camera();
    //when
    let res = cam.set_readout_mode(1_u32);
    //then
    assert!(res.is_err());
    assert_eq!(
        res.err().unwrap(),
        QHYError::Sdk {
            op: "set_readout_mode"
        }
    );
}

#[test]
fn get_model_success() {
    let _mock = super::mock_guard();
    //given
    let ctx = GetQHYCCDModel_context();
    ctx.expect().times(1).returning_st(|_handle, model| unsafe {
        let cam_model = "QHY178M\0";
        model.copy_from(cam_model.as_ptr() as *const c_char, cam_model.len());

        QHYCCD_SUCCESS
    });
    let cam = new_camera();
    //when
    let res = cam.get_model();
    //then
    assert!(res.is_ok());
    assert_eq!(res.unwrap(), "QHY178M");
}

#[test]
fn get_model_fail() {
    let _mock = super::mock_guard();
    //given
    let ctx = GetQHYCCDModel_context();
    ctx.expect().times(1).return_const(QHYCCD_ERROR);
    let cam = new_camera();
    //when
    let res = cam.get_model();
    //then
    assert!(res.is_err());
}

#[test]
fn get_model_utf8_error() {
    let _mock = super::mock_guard();
    //given
    let ctx = GetQHYCCDModel_context();
    ctx.expect().times(1).returning_st(|_handle, model| unsafe {
        let cam_model = b"\xc3\x28\0";
        model.copy_from(cam_model.as_ptr() as *const c_char, cam_model.len());

        QHYCCD_SUCCESS
    });
    let cam = new_camera();
    //when
    let res = cam.get_model();
    //then
    assert!(res.is_err());
    // Built at runtime (not a literal) so the `invalid_from_utf8` lint does not
    // fire; matches the bytes the mocked SDK returns.
    let invalid_utf8 = vec![0xc3_u8, 0x28];
    assert_eq!(
        res.err().unwrap(),
        QHYError::InvalidUtf8(std::str::from_utf8(&invalid_utf8).unwrap_err())
    );
}

#[test]
fn init_success() {
    let _mock = super::mock_guard();
    //given
    let ctx = InitQHYCCD_context();
    ctx.expect()
        .withf_st(|handle| *handle == TEST_HANDLE)
        .times(1)
        .return_const_st(QHYCCD_SUCCESS);
    let cam = new_camera();
    //when
    let res = cam.init();
    //then
    assert!(res.is_ok());
}

#[test]
fn init_fail() {
    let _mock = super::mock_guard();
    //given
    let ctx = InitQHYCCD_context();
    ctx.expect()
        .withf_st(|handle| *handle == TEST_HANDLE)
        .times(1)
        .return_const_st(QHYCCD_ERROR);
    let cam = new_camera();
    //when
    let res = cam.init();
    //then
    assert!(res.is_err());
    assert_eq!(res.err().unwrap(), QHYError::Sdk { op: "init_camera" });
}

#[test]
fn get_firmware_version_success() {
    let _mock = super::mock_guard();
    //given
    let ctx = GetQHYCCDFWVersion_context();
    ctx.expect()
        .times(1)
        .returning_st(|_handle, version| unsafe {
            let fw_version = b"\x01\x23\0";
            version.copy_from(fw_version.as_ptr(), fw_version.len());

            QHYCCD_SUCCESS
        });
    let cam = new_camera();
    //when
    let res = cam.get_firmware_version();
    //then
    assert!(res.is_ok());
    assert_eq!(res.unwrap(), "Firmware version: 2016_1_35");
    // Drop the first camera (closing its handle) before building a second one:
    // each `new_camera()` carries its own `CloseQHYCCD` mock context, and two live
    // at once would clear each other's expectation on drop.
    drop(cam);

    //given
    let ctx = GetQHYCCDFWVersion_context();
    ctx.expect()
        .times(1)
        .returning_st(|_handle, version| unsafe {
            let fw_version = b"\xA1\x11\0";
            version.copy_from(fw_version.as_ptr(), fw_version.len());

            QHYCCD_SUCCESS
        });
    let cam = new_camera();
    //when
    let res = cam.get_firmware_version();
    //then
    assert!(res.is_ok());
    assert_eq!(res.unwrap(), "Firmware version: 2010_1_17");
}

#[test]
fn get_firmware_version_fail() {
    let _mock = super::mock_guard();
    //given
    let ctx = GetQHYCCDFWVersion_context();
    ctx.expect()
        .withf_st(|handle, _version| *handle == TEST_HANDLE)
        .times(1)
        .return_const_st(QHYCCD_ERROR);
    let cam = new_camera();
    //when
    let res = cam.get_firmware_version();
    //then
    assert!(res.is_err());
    assert_eq!(
        res.err().unwrap(),
        QHYError::Sdk {
            op: "get_firmware_version"
        }
    );
}

#[test]
fn get_number_of_readout_modes_success() {
    let _mock = super::mock_guard();
    //given
    let ctx = GetQHYCCDNumberOfReadModes_context();
    ctx.expect()
        .withf_st(|handle, _number| *handle == TEST_HANDLE)
        .times(1)
        .returning_st(|_handle, number| unsafe {
            *number = 2;
            QHYCCD_SUCCESS
        });
    let cam = new_camera();
    //when
    let res = cam.get_number_of_readout_modes();
    //then
    assert!(res.is_ok());
    assert_eq!(res.unwrap(), 2);
}

#[test]
fn get_number_of_readout_modes_fail() {
    let _mock = super::mock_guard();
    //given
    let ctx = GetQHYCCDNumberOfReadModes_context();
    ctx.expect()
        .withf_st(|handle, _number| *handle == TEST_HANDLE)
        .times(1)
        .return_const_st(QHYCCD_ERROR);
    let cam = new_camera();
    //when
    let res = cam.get_number_of_readout_modes();
    //then
    assert!(res.is_err());
    assert_eq!(
        res.err().unwrap(),
        QHYError::Sdk {
            op: "get_number_of_readout_modes"
        }
    );
}

#[test]
fn get_readout_mode_name_success() {
    let _mock = super::mock_guard();
    //given
    let ctx = GetQHYCCDReadModeName_context();
    ctx.expect()
        .withf_st(|handle, _index, _mode| *handle == TEST_HANDLE)
        .times(1)
        .returning_st(|_handle, _index, mode| unsafe {
            let read_mode = "STANDARD MODE\0";
            mode.copy_from(read_mode.as_ptr() as *const c_char, read_mode.len());

            QHYCCD_SUCCESS
        });
    let cam = new_camera();
    //when
    let res = cam.get_readout_mode_name(0);
    //then
    assert!(res.is_ok());
    assert_eq!(res.unwrap(), "STANDARD MODE");
}

#[test]
fn get_readout_mode_name_fail() {
    let _mock = super::mock_guard();
    //given
    let ctx = GetQHYCCDReadModeName_context();
    ctx.expect()
        .withf_st(|handle, _index, _mode| *handle == TEST_HANDLE)
        .times(1)
        .return_const(QHYCCD_ERROR);
    let cam = new_camera();
    //when
    let res = cam.get_readout_mode_name(0);
    //then
    assert!(res.is_err());
    assert_eq!(
        res.err().unwrap(),
        QHYError::Sdk {
            op: "get_readout_mode_name"
        }
    );
}

#[test]
fn get_readout_mode_name_utf8_error() {
    let _mock = super::mock_guard();
    //given
    let ctx = GetQHYCCDReadModeName_context();
    ctx.expect()
        .withf_st(|handle, _index, _mode| *handle == TEST_HANDLE)
        .times(1)
        .returning_st(|_handle, _index, mode| unsafe {
            let read_mode = b"\xc3\x28\0";
            mode.copy_from(read_mode.as_ptr() as *const c_char, read_mode.len());

            QHYCCD_SUCCESS
        });
    let cam = new_camera();
    //when
    let res = cam.get_readout_mode_name(0);
    //then
    assert!(res.is_err());
    // Built at runtime (not a literal) so the `invalid_from_utf8` lint does not
    // fire; matches the bytes the mocked SDK returns.
    let invalid_utf8 = vec![0xc3_u8, 0x28];
    assert_eq!(
        res.err().unwrap(),
        QHYError::InvalidUtf8(std::str::from_utf8(&invalid_utf8).unwrap_err())
    );
}

#[test]
fn get_readout_mode_resolution_success() {
    let _mock = super::mock_guard();
    //given
    let ctx = GetQHYCCDReadModeResolution_context();
    ctx.expect()
        .withf_st(|handle, _index, _width, _height| *handle == TEST_HANDLE)
        .times(1)
        .returning_st(|_handle, _index, width, height| unsafe {
            *width = 1024;
            *height = 768;

            QHYCCD_SUCCESS
        });
    let cam = new_camera();
    //when
    let res = cam.get_readout_mode_resolution(0);
    //then
    assert!(res.is_ok());
    assert_eq!(res.unwrap(), (1024, 768));
}

#[test]
fn get_readout_mode_resolution_fail() {
    let _mock = super::mock_guard();
    //given
    let ctx = GetQHYCCDReadModeResolution_context();
    ctx.expect()
        .withf_st(|handle, _index, _width, _height| *handle == TEST_HANDLE)
        .times(1)
        .return_const(QHYCCD_ERROR);
    let cam = new_camera();
    //when
    let res = cam.get_readout_mode_resolution(0);
    //then
    assert!(res.is_err());
    assert_eq!(
        res.err().unwrap(),
        QHYError::Sdk {
            op: "get_readout_mode_resolution"
        }
    );
}

#[test]
fn get_readout_mode_success() {
    let _mock = super::mock_guard();
    //given
    let ctx = GetQHYCCDReadMode_context();
    ctx.expect()
        .withf_st(|handle, _mode| *handle == TEST_HANDLE)
        .times(1)
        .returning_st(|_handle, mode| unsafe {
            *mode = 1;
            QHYCCD_SUCCESS
        });
    let cam = new_camera();
    //when
    let res = cam.get_readout_mode();
    //then
    assert!(res.is_ok());
    assert_eq!(res.unwrap(), 1);
}

#[test]
fn get_readout_mode_fail() {
    let _mock = super::mock_guard();
    //given
    let ctx = GetQHYCCDReadMode_context();
    ctx.expect()
        .withf_st(|handle, _mode| *handle == TEST_HANDLE)
        .times(1)
        .return_const_st(QHYCCD_ERROR);
    let cam = new_camera();
    //when
    let res = cam.get_readout_mode();
    //then
    assert!(res.is_err());
    assert_eq!(
        res.err().unwrap(),
        QHYError::Sdk {
            op: "get_readout_mode"
        }
    );
}

#[test]
fn get_type_success() {
    let _mock = super::mock_guard();
    //given
    let ctx = GetQHYCCDType_context();
    ctx.expect()
        .withf_st(|handle| *handle == TEST_HANDLE)
        .times(1)
        .return_const_st(42_u32);
    let cam = new_camera();
    //when
    let res = cam.get_type();
    //then
    assert!(res.is_ok());
    assert_eq!(res.unwrap(), 42_u32);
}

#[test]
fn get_type_fail() {
    let _mock = super::mock_guard();
    //given
    let ctx = GetQHYCCDType_context();
    ctx.expect()
        .withf_st(|handle| *handle == TEST_HANDLE)
        .times(1)
        .return_const_st(QHYCCD_ERROR);
    let cam = new_camera();
    //when
    let res = cam.get_type();
    //then
    assert!(res.is_err());
    assert_eq!(res.err().unwrap(), QHYError::Sdk { op: "get_type" });
}

#[test]
fn set_bin_mode_success() {
    let _mock = super::mock_guard();
    //given
    let ctx = SetQHYCCDBinMode_context();
    ctx.expect()
        .withf_st(|handle, bin_x, bin_y| {
            *handle == TEST_HANDLE && *bin_x == 2_u32 && *bin_y == 2_u32
        })
        .times(1)
        .return_const_st(QHYCCD_SUCCESS);
    let cam = new_camera();
    //when
    let res = cam.set_bin_mode(2, 2);
    //then
    assert!(res.is_ok());
}

#[test]
fn set_bin_mode_fail() {
    let _mock = super::mock_guard();
    //given
    let ctx = SetQHYCCDBinMode_context();
    ctx.expect()
        .withf_st(|handle, bin_x, bin_y| {
            *handle == TEST_HANDLE && *bin_x == 2_u32 && *bin_y == 2_u32
        })
        .times(1)
        .return_const_st(QHYCCD_ERROR);
    let cam = new_camera();
    //when
    let res = cam.set_bin_mode(2, 2);
    //then
    assert!(res.is_err());
    assert_eq!(res.err().unwrap(), QHYError::Sdk { op: "set_bin_mode" });
}

#[test]
fn set_debayer_success() {
    let _mock = super::mock_guard();
    //given
    let ctx = SetQHYCCDDebayerOnOff_context();
    ctx.expect()
        .withf_st(|handle, on| *handle == TEST_HANDLE && *on)
        .times(1)
        .return_const_st(QHYCCD_SUCCESS);
    let cam = new_camera();
    //when
    let res = cam.set_debayer(true);
    //then
    assert!(res.is_ok());
}

#[test]
fn set_debayer_fail() {
    let _mock = super::mock_guard();
    //given
    let ctx = SetQHYCCDDebayerOnOff_context();
    ctx.expect()
        .withf_st(|handle, on| *handle == TEST_HANDLE && *on)
        .times(1)
        .return_const_st(QHYCCD_ERROR);
    let cam = new_camera();
    //when
    let res = cam.set_debayer(true);
    //then
    assert!(res.is_err());
    assert_eq!(res.err().unwrap(), QHYError::Sdk { op: "set_debayer" });
}

#[test]
fn set_roi_success() {
    let _mock = super::mock_guard();
    //given
    let ctx = SetQHYCCDResolution_context();
    ctx.expect()
        .withf_st(|handle, start_x, stary_y, width, height| {
            *handle == TEST_HANDLE
                && *start_x == 0_u32
                && *stary_y == 0_u32
                && *width == 1024_u32
                && *height == 768_u32
        })
        .times(1)
        .return_const_st(QHYCCD_SUCCESS);
    let cam = new_camera();
    //when
    let res = cam.set_roi(CCDChipArea {
        start_x: 0,
        start_y: 0,
        width: 1024,
        height: 768,
    });
    //then
    assert!(res.is_ok());
}

#[test]
fn set_roi_fail() {
    let _mock = super::mock_guard();
    //given
    let ctx = SetQHYCCDResolution_context();
    ctx.expect()
        .withf_st(|handle, start_x, stary_y, width, height| {
            *handle == TEST_HANDLE
                && *start_x == 0_u32
                && *stary_y == 0_u32
                && *width == 1024_u32
                && *height == 768_u32
        })
        .times(1)
        .return_const_st(QHYCCD_ERROR);
    let cam = new_camera();
    //when
    let res = cam.set_roi(CCDChipArea {
        start_x: 0,
        start_y: 0,
        width: 1024,
        height: 768,
    });
    //then
    assert!(res.is_err());
    assert_eq!(res.err().unwrap(), QHYError::Sdk { op: "set_roi" });
}

#[test]
fn begin_live_success() {
    let _mock = super::mock_guard();
    //given
    let ctx = BeginQHYCCDLive_context();
    ctx.expect()
        .withf_st(|handle| *handle == TEST_HANDLE)
        .times(1)
        .return_const_st(QHYCCD_SUCCESS);
    let cam = new_camera();
    //when
    let res = cam.begin_live();
    //then
    assert!(res.is_ok());
}

#[test]
fn begin_live_fail() {
    let _mock = super::mock_guard();
    //given
    let ctx = BeginQHYCCDLive_context();
    ctx.expect()
        .withf_st(|handle| *handle == TEST_HANDLE)
        .times(1)
        .return_const_st(QHYCCD_ERROR);
    let cam = new_camera();
    //when
    let res = cam.begin_live();
    //then
    assert!(res.is_err());
    assert_eq!(res.err().unwrap(), QHYError::Sdk { op: "begin_live" });
}

#[test]
fn end_live_success() {
    let _mock = super::mock_guard();
    //given
    let ctx = StopQHYCCDLive_context();
    ctx.expect()
        .withf_st(|handle| *handle == TEST_HANDLE)
        .times(1)
        .return_const_st(QHYCCD_SUCCESS);
    let cam = new_camera();
    //when
    let res = cam.end_live();
    //then
    assert!(res.is_ok());
}

#[test]
fn end_live_fail() {
    let _mock = super::mock_guard();
    //given
    let ctx = StopQHYCCDLive_context();
    ctx.expect()
        .withf_st(|handle| *handle == TEST_HANDLE)
        .times(1)
        .return_const_st(QHYCCD_ERROR);
    let cam = new_camera();
    //when
    let res = cam.end_live();
    //then
    assert!(res.is_err());
    assert_eq!(res.err().unwrap(), QHYError::Sdk { op: "end_live" });
}

#[test]
fn get_image_size_success() {
    let _mock = super::mock_guard();
    //given
    let ctx = GetQHYCCDMemLength_context();
    ctx.expect()
        .withf_st(|handle| *handle == TEST_HANDLE)
        .times(1)
        .return_const_st(42_u32);
    let cam = new_camera();
    //when
    let res = cam.get_image_size();
    //then
    assert!(res.is_ok());
    assert_eq!(res.unwrap(), 42);
}

#[test]
fn get_image_size_fail() {
    let _mock = super::mock_guard();
    //given
    let ctx = GetQHYCCDMemLength_context();
    ctx.expect()
        .withf_st(|handle| *handle == TEST_HANDLE)
        .times(1)
        .return_const_st(QHYCCD_ERROR);
    let cam = new_camera();
    //when
    let res = cam.get_image_size();
    //then
    assert!(res.is_err());
    assert_eq!(
        res.err().unwrap(),
        QHYError::Sdk {
            op: "get_image_size"
        }
    );
}

#[test]
fn get_live_frame_success() {
    let _mock = super::mock_guard();
    //given
    let ctx = GetQHYCCDLiveFrame_context();
    ctx.expect()
        .withf_st(|handle, _width, _height, _bpp, _channels, _buffer| *handle == TEST_HANDLE)
        .times(1)
        .returning_st(|_handle, width, height, bpp, channels, buffer| unsafe {
            *width = 2;
            *height = 2;
            *bpp = 8;
            *channels = 1;
            let test_image = b"\x01\x02\x03\x04";
            buffer.copy_from(test_image.as_ptr(), 4);
            QHYCCD_SUCCESS
        });
    let cam = new_camera();
    //when
    let res = cam.get_live_frame(4);
    //then
    assert!(res.is_ok());
    assert_eq!(
        res.unwrap(),
        ImageData {
            data: vec![0x01, 0x02, 0x03, 0x04],
            width: 2,
            height: 2,
            bits_per_pixel: 8,
            channels: 1
        }
    )
}

#[test]
fn get_live_frame_fail() {
    let _mock = super::mock_guard();
    //given
    let ctx = GetQHYCCDLiveFrame_context();
    ctx.expect()
        .withf_st(|handle, _width, _height, _bpp, _channels, _buffer| *handle == TEST_HANDLE)
        .times(1)
        .return_const_st(QHYCCD_ERROR);
    let cam = new_camera();
    //when
    let res = cam.get_live_frame(4);
    //then
    assert!(res.is_err());
    assert_eq!(
        res.err().unwrap(),
        QHYError::Sdk {
            op: "get_live_frame"
        }
    );
}

#[test]
fn get_single_frame_success() {
    let _mock = super::mock_guard();
    //given
    let ctx = GetQHYCCDSingleFrame_context();
    ctx.expect()
        .withf_st(|handle, _width, _height, _bpp, _channels, _buffer| *handle == TEST_HANDLE)
        .times(1)
        .returning_st(|_handle, width, height, bpp, channels, buffer| unsafe {
            *width = 2;
            *height = 2;
            *bpp = 8;
            *channels = 1;
            let test_image = b"\x01\x02\x03\x04";
            buffer.copy_from(test_image.as_ptr(), 4);
            QHYCCD_SUCCESS
        });
    let cam = new_camera();
    //when
    let res = cam.get_single_frame(4);
    //then
    assert!(res.is_ok());
    assert_eq!(
        res.unwrap(),
        ImageData {
            data: vec![0x01, 0x02, 0x03, 0x04],
            width: 2,
            height: 2,
            bits_per_pixel: 8,
            channels: 1
        }
    )
}

#[test]
fn get_single_frame_fail() {
    let _mock = super::mock_guard();
    //given
    let ctx = GetQHYCCDSingleFrame_context();
    ctx.expect()
        .withf_st(|handle, _width, _height, _bpp, _channels, _buffer| *handle == TEST_HANDLE)
        .times(1)
        .return_const_st(QHYCCD_ERROR);
    let cam = new_camera();
    //when
    let res = cam.get_single_frame(4);
    //then
    assert!(res.is_err());
    assert_eq!(
        res.err().unwrap(),
        QHYError::Sdk {
            op: "get_single_frame"
        }
    );
}

#[test]
fn get_overscan_area_success() {
    let _mock = super::mock_guard();
    //given
    let ctx = GetQHYCCDOverScanArea_context();
    ctx.expect()
        .withf_st(|handle, _start_x, _start_y, _width, _height| *handle == TEST_HANDLE)
        .times(1)
        .returning_st(|_handle, start_x, start_y, width, height| unsafe {
            *start_x = 2;
            *start_y = 5;
            *width = 1024;
            *height = 768;
            QHYCCD_SUCCESS
        });
    let cam = new_camera();
    //when
    let res = cam.get_overscan_area();
    //then
    assert!(res.is_ok());
    assert_eq!(
        res.unwrap(),
        CCDChipArea {
            start_x: 2,
            start_y: 5,
            width: 1024,
            height: 768
        }
    )
}

#[test]
fn get_overscan_area_fail() {
    let _mock = super::mock_guard();
    //given
    let ctx = GetQHYCCDOverScanArea_context();
    ctx.expect()
        .withf_st(|handle, _start_x, _start_y, _width, _height| *handle == TEST_HANDLE)
        .times(1)
        .return_const_st(QHYCCD_ERROR);
    let cam = new_camera();
    //when
    let res = cam.get_overscan_area();
    //then
    assert!(res.is_err());
    assert_eq!(
        res.err().unwrap(),
        QHYError::Sdk {
            op: "get_overscan_area"
        }
    );
}

#[test]
fn get_effective_area_success() {
    let _mock = super::mock_guard();
    //given
    let ctx = GetQHYCCDEffectiveArea_context();
    ctx.expect()
        .withf_st(|handle, _start_x, _start_y, _width, _height| *handle == TEST_HANDLE)
        .times(1)
        .returning_st(|_handle, start_x, start_y, width, height| unsafe {
            *start_x = 0;
            *start_y = 0;
            *width = 1024;
            *height = 768;
            QHYCCD_SUCCESS
        });
    let cam = new_camera();
    //when
    let res = cam.get_effective_area();
    //then
    assert!(res.is_ok());
    assert_eq!(
        res.unwrap(),
        CCDChipArea {
            start_x: 0,
            start_y: 0,
            width: 1024,
            height: 768
        }
    )
}

#[test]
fn get_effective_area_fail() {
    let _mock = super::mock_guard();
    //given
    let ctx = GetQHYCCDEffectiveArea_context();
    ctx.expect()
        .withf_st(|handle, _start_x, _start_y, _width, _height| *handle == TEST_HANDLE)
        .times(1)
        .return_const_st(QHYCCD_ERROR);
    let cam = new_camera();
    //when
    let res = cam.get_effective_area();
    //then
    assert!(res.is_err());
    assert_eq!(
        res.err().unwrap(),
        QHYError::Sdk {
            op: "get_effective_area"
        }
    );
}

#[test]
fn start_single_frame_exposure_success() {
    let _mock = super::mock_guard();
    //given
    let ctx = ExpQHYCCDSingleFrame_context();
    ctx.expect()
        .withf_st(|handle| *handle == TEST_HANDLE)
        .times(1)
        .return_const_st(QHYCCD_SUCCESS);
    let cam = new_camera();
    //when
    let res = cam.start_single_frame_exposure();
    //then
    assert!(res.is_ok());
}

#[test]
fn start_single_frame_exposure_fail() {
    let _mock = super::mock_guard();
    //given
    let ctx = ExpQHYCCDSingleFrame_context();
    ctx.expect()
        .withf_st(|handle| *handle == TEST_HANDLE)
        .times(1)
        .return_const_st(QHYCCD_ERROR);
    let cam = new_camera();
    //when
    let res = cam.start_single_frame_exposure();
    //then
    assert!(res.is_err());
    assert_eq!(
        res.err().unwrap(),
        QHYError::Sdk {
            op: "start_single_frame_exposure"
        }
    );
}

#[test]
fn get_remaining_exposure_us_success() {
    let _mock = super::mock_guard();
    //given
    let ctx = GetQHYCCDExposureRemaining_context();
    ctx.expect()
        .withf_st(|handle| *handle == TEST_HANDLE)
        .times(1)
        .return_const_st(42_u32);
    let cam = new_camera();
    //when
    let res = cam.get_remaining_exposure_us();
    //then
    assert!(res.is_ok());
    assert_eq!(res.unwrap(), 0);
    //given
    ctx.expect()
        .withf_st(|handle| *handle == TEST_HANDLE)
        .times(1)
        .return_const_st(42000_u32);
    //when
    let res = cam.get_remaining_exposure_us();
    //then
    assert!(res.is_ok());
    assert_eq!(res.unwrap(), 42000);
}

#[test]
fn get_remaining_exposure_us_fail() {
    let _mock = super::mock_guard();
    //given
    let ctx = GetQHYCCDExposureRemaining_context();
    ctx.expect()
        .withf_st(|handle| *handle == TEST_HANDLE)
        .times(1)
        .return_const_st(QHYCCD_ERROR);
    let cam = new_camera();
    //when
    let res = cam.get_remaining_exposure_us();
    //then
    assert!(res.is_err());
    assert_eq!(
        res.err().unwrap(),
        QHYError::Sdk {
            op: "get_remaining_exposure_us"
        }
    );
}

#[test]
fn stop_exposure_success() {
    let _mock = super::mock_guard();
    //given
    let ctx = CancelQHYCCDExposing_context();
    ctx.expect()
        .withf_st(|handle| *handle == TEST_HANDLE)
        .times(1)
        .return_const_st(QHYCCD_SUCCESS);
    let cam = new_camera();
    //when
    let res = cam.stop_exposure();
    //then
    assert!(res.is_ok());
}

#[test]
fn stop_exposure_fail() {
    let _mock = super::mock_guard();
    //given
    let ctx = CancelQHYCCDExposing_context();
    ctx.expect()
        .withf_st(|handle| *handle == TEST_HANDLE)
        .times(1)
        .return_const_st(QHYCCD_ERROR);
    let cam = new_camera();
    //when
    let res = cam.stop_exposure();
    //then
    assert!(res.is_err());
    assert_eq!(
        res.err().unwrap(),
        QHYError::Sdk {
            op: "stop_exposure"
        }
    );
}

#[test]
fn abort_exposure_and_readout_success() {
    let _mock = super::mock_guard();
    //given
    let ctx = CancelQHYCCDExposingAndReadout_context();
    ctx.expect()
        .withf_st(|handle| *handle == TEST_HANDLE)
        .times(1)
        .return_const_st(QHYCCD_SUCCESS);
    let cam = new_camera();
    //when
    let res = cam.abort_exposure_and_readout();
    //then
    assert!(res.is_ok());
}

#[test]
fn abort_exposure_and_readout_fail() {
    let _mock = super::mock_guard();
    //given
    let ctx = CancelQHYCCDExposingAndReadout_context();
    ctx.expect()
        .withf_st(|handle| *handle == TEST_HANDLE)
        .times(1)
        .return_const_st(QHYCCD_ERROR);
    let cam = new_camera();
    //when
    let res = cam.abort_exposure_and_readout();
    //then
    assert!(res.is_err());
    assert_eq!(
        res.err().unwrap(),
        QHYError::Sdk {
            op: "abort_exposure_and_readout"
        }
    );
}

#[test]
fn is_control_available_success_some() {
    let _mock = super::mock_guard();
    //given
    let ctx = IsQHYCCDControlAvailable_context();
    ctx.expect()
        .withf_st(|handle, _control| *handle == TEST_HANDLE)
        .times(1)
        .return_const_st(QHYCCD_SUCCESS);
    let cam = new_camera();
    //when
    let res = cam.is_control_available(ControlType::Brightness);
    //then
    assert!(res.is_some());
    assert_eq!(res.unwrap(), QHYCCD_SUCCESS)
}

#[test]
fn is_control_available_success_none() {
    let _mock = super::mock_guard();
    //given
    let ctx = IsQHYCCDControlAvailable_context();
    ctx.expect()
        .withf_st(|handle, _control| *handle == TEST_HANDLE)
        .times(1)
        .return_const_st(QHYCCD_ERROR);
    let cam = new_camera();
    //when
    let res = cam.is_control_available(ControlType::Brightness);
    //then
    assert!(res.is_none());
}

#[test]
fn get_ccd_info_success() {
    let _mock = super::mock_guard();
    //given
    let ctx = GetQHYCCDChipInfo_context();
    ctx.expect()
        .withf_st(
            |handle, _chipw, _chiph, _imagew, _imageh, _pixelw, _pixelh, _bpp| {
                *handle == TEST_HANDLE
            },
        )
        .times(1)
        .returning_st(
            |_handle, chipw, chiph, imagew, imageh, pixelw, pixelh, bpp| unsafe {
                *chipw = 3124.1;
                *chiph = 500.5;
                *imagew = 1024;
                *imageh = 768;
                *pixelw = 2.4;
                *pixelh = 2.4;
                *bpp = 16;
                QHYCCD_SUCCESS
            },
        );
    let cam = new_camera();
    //when
    let res = cam.get_ccd_info();
    //then
    assert!(res.is_ok());
    assert_eq!(
        res.unwrap(),
        CCDChipInfo {
            chip_width: 3124.1,
            chip_height: 500.5,
            image_width: 1024,
            image_height: 768,
            pixel_width: 2.4,
            pixel_height: 2.4,
            bits_per_pixel: 16
        }
    )
}

#[test]
fn get_ccd_info_fail() {
    let _mock = super::mock_guard();
    //given
    let ctx = GetQHYCCDChipInfo_context();
    ctx.expect()
        .withf_st(
            |handle, _chipw, _chiph, _imagew, _imageh, _pixelw, _pixelh, _bpp| {
                *handle == TEST_HANDLE
            },
        )
        .times(1)
        .return_const_st(QHYCCD_ERROR);
    let cam = new_camera();
    //when
    let res = cam.get_ccd_info();
    //then
    assert!(res.is_err());
    assert_eq!(res.err().unwrap(), QHYError::Sdk { op: "get_ccd_info" });
}

#[test]
fn set_bit_mode_success() {
    let _mock = super::mock_guard();
    //given
    let ctx = SetQHYCCDBitsMode_context();
    ctx.expect()
        .withf_st(|handle, mode| *handle == TEST_HANDLE && *mode == 0_u32)
        .times(1)
        .return_const_st(QHYCCD_SUCCESS);
    let cam = new_camera();
    //when
    let res = cam.set_bit_mode(0);
    //then
    assert!(res.is_ok());
}

#[test]
fn set_bit_mode_fail() {
    let _mock = super::mock_guard();
    //given
    let ctx = SetQHYCCDBitsMode_context();
    ctx.expect()
        .withf_st(|handle, mode| *handle == TEST_HANDLE && *mode == 0_u32)
        .times(1)
        .return_const_st(QHYCCD_ERROR);
    let cam = new_camera();
    //when
    let res = cam.set_bit_mode(0);
    //then
    assert!(res.is_err());
    assert_eq!(res.err().unwrap(), QHYError::Sdk { op: "set_bit_mode" });
}

#[test]
fn get_parameter_success() {
    let _mock = super::mock_guard();
    //given
    let ctx = GetQHYCCDParam_context();
    ctx.expect()
        .withf_st(|handle, control| {
            *handle == TEST_HANDLE && *control == ControlType::CfwSlotsNum.to_raw()
        })
        .times(1)
        .return_const_st(5.0);
    let cam = new_camera();
    //when
    let res = cam.get_parameter(ControlType::CfwSlotsNum);
    //then
    assert!(res.is_ok());
    assert_eq!(res.unwrap(), 5.0);
}

#[test]
fn get_parameter_fail() {
    let _mock = super::mock_guard();
    //given
    let ctx = GetQHYCCDParam_context();
    ctx.expect()
        .withf_st(|handle, control| {
            *handle == TEST_HANDLE && *control == ControlType::CfwSlotsNum.to_raw()
        })
        .once()
        .return_const_st(QHYCCD_ERROR_F64);
    let cam = new_camera();
    //when
    let res = cam.get_parameter(ControlType::CfwSlotsNum);
    //then
    assert!(res.is_err());
    assert_eq!(
        res.err().unwrap(),
        QHYError::GetParameter {
            control: ControlType::CfwSlotsNum
        }
    );
}

#[test]
fn get_parameter_min_max_step_success() {
    let _mock = super::mock_guard();
    //given
    let ctx = GetQHYCCDParamMinMaxStep_context();
    ctx.expect()
        .withf_st(|handle, control, _min, _max, _step| {
            *handle == TEST_HANDLE && *control == ControlType::Exposure.to_raw()
        })
        .once()
        .returning_st(|_handle, _control, min, max, step| unsafe {
            *min = 0.0;
            *max = 100.0;
            *step = 0.1;
            QHYCCD_SUCCESS
        });
    let cam = new_camera();
    //when
    let res = cam.get_parameter_min_max_step(ControlType::Exposure);
    //then
    assert!(res.is_ok());
    assert_eq!(res.unwrap(), (0.0, 100.0, 0.1));
}

#[test]
fn get_parameter_min_max_step_fail() {
    let _mock = super::mock_guard();
    //given
    let ctx = GetQHYCCDParamMinMaxStep_context();
    ctx.expect()
        .withf_st(|handle, control, _min, _max, _step| {
            *handle == TEST_HANDLE && *control == ControlType::Exposure.to_raw()
        })
        .once()
        .return_const_st(QHYCCD_ERROR);
    let cam = new_camera();
    //when
    let res = cam.get_parameter_min_max_step(ControlType::Exposure);
    //then
    assert!(res.is_err());
    assert_eq!(
        res.err().unwrap(),
        QHYError::GetMinMaxStep {
            control: ControlType::Exposure
        }
    );
}

#[test]
fn set_parameter_success() {
    let _mock = super::mock_guard();
    //given
    let ctx = SetQHYCCDParam_context();
    ctx.expect()
        .withf_st(|handle, control, value| {
            *handle == TEST_HANDLE
                && *control == ControlType::TransferBit.to_raw()
                && *value == 16.0
        })
        .times(1)
        .return_const_st(QHYCCD_SUCCESS);
    let cam = new_camera();
    //when
    let res = cam.set_parameter(ControlType::TransferBit, 16.0);
    //then
    assert!(res.is_ok());
}

#[test]
fn set_parameter_fail() {
    let _mock = super::mock_guard();
    //given
    let ctx = SetQHYCCDParam_context();
    ctx.expect()
        .withf_st(|handle, control, value| {
            *handle == TEST_HANDLE
                && *control == ControlType::TransferBit.to_raw()
                && *value == 16.0
        })
        .times(1)
        .return_const_st(QHYCCD_ERROR);
    let cam = new_camera();
    //when
    let res = cam.set_parameter(ControlType::TransferBit, 16.0);
    //then
    assert!(res.is_err());
    assert_eq!(
        res.err().unwrap(),
        QHYError::Sdk {
            op: "set_parameter"
        }
    );
}

#[test]
fn set_if_available_success() {
    let _mock = super::mock_guard();
    //given
    let ctx_get = IsQHYCCDControlAvailable_context();
    ctx_get
        .expect()
        .withf_st(|handle, control| {
            *handle == TEST_HANDLE && *control == ControlType::TransferBit.to_raw()
        })
        .times(1)
        .return_const_st(QHYCCD_SUCCESS);

    let ctx_set = SetQHYCCDParam_context();
    ctx_set
        .expect()
        .withf_st(|handle, control, value| {
            *handle == TEST_HANDLE
                && *control == ControlType::TransferBit.to_raw()
                && *value == 16.0
        })
        .times(1)
        .return_const_st(QHYCCD_SUCCESS);
    let cam = new_camera();
    //when
    let res = cam.set_if_available(ControlType::TransferBit, 16.0);
    //then
    assert!(res.is_ok());
}

#[test]
fn set_if_available_fail() {
    let _mock = super::mock_guard();
    //given
    let ctx_get = IsQHYCCDControlAvailable_context();
    ctx_get
        .expect()
        .withf_st(|handle, control| {
            *handle == TEST_HANDLE && *control == ControlType::TransferBit.to_raw()
        })
        .times(1)
        .return_const_st(QHYCCD_ERROR);

    /*     let ctx_set = SetQHYCCDParam_context();
       ctx_set
           .expect()
           .withf_st(|handle, control, value| {
               *handle == TEST_HANDLE && *control == ControlType::TransferBit.to_raw() && *value == 16.0
           })
           .times(1)
           .return_const_st(QHYCCD_ERROR);
    */
    let cam = new_camera();
    //when
    let res = cam.set_if_available(ControlType::TransferBit, 16.0);
    //then
    assert!(res.is_err());
    assert_eq!(
        res.err().unwrap(),
        QHYError::IsControlAvailable {
            control: ControlType::TransferBit
        }
    );
    // Drop the first camera (closing its handle) before building a second one:
    // each `new_camera()` carries its own `CloseQHYCCD` mock context, and two live
    // at once would clear each other's expectation on drop.
    drop(cam);

    //given
    let ctx_get = IsQHYCCDControlAvailable_context();
    ctx_get
        .expect()
        .withf_st(|handle, control| {
            *handle == TEST_HANDLE && *control == ControlType::TransferBit.to_raw()
        })
        .times(1)
        .return_const_st(QHYCCD_SUCCESS);

    let ctx_set = SetQHYCCDParam_context();
    ctx_set
        .expect()
        .withf_st(|handle, control, value| {
            *handle == TEST_HANDLE
                && *control == ControlType::TransferBit.to_raw()
                && *value == 16.0
        })
        .times(1)
        .return_const_st(QHYCCD_ERROR);
    let cam = new_camera();
    //when
    let res = cam.set_if_available(ControlType::TransferBit, 16.0);
    //then
    assert!(res.is_err());
    assert_eq!(
        res.err().unwrap(),
        QHYError::Sdk {
            op: "set_parameter"
        }
    );
}

#[test]
fn open_success() {
    let _mock = super::mock_guard();
    //given
    let ctx_open = OpenQHYCCD_context();
    ctx_open.expect().times(1).return_const_st(TEST_HANDLE);
    // The opened handle is closed by `HandleCell::Drop` when `cam` drops; declare
    // the expectation (and `cam` last) so it is live at that drop.
    let ctx_close = CloseQHYCCD_context();
    ctx_close.expect().return_const_st(QHYCCD_SUCCESS);
    let cam = Camera::new("test_camera".to_owned());
    //when
    let res = cam.open();
    //then
    assert!(res.is_ok());
    assert_eq!(cam.id(), "test_camera".to_owned());
}

#[test]
fn open_already_open() {
    let _mock = super::mock_guard();
    //given
    let ctx_open = OpenQHYCCD_context();
    ctx_open.expect().times(1).return_const_st(TEST_HANDLE);
    let ctx_close = CloseQHYCCD_context();
    ctx_close.expect().return_const_st(QHYCCD_SUCCESS);
    let cam = Camera::new("test_camera".to_owned());
    let _res = cam.open();
    //when
    let res = cam.open();
    //then
    assert!(res.is_ok());
}

#[test]
fn open_fail() {
    let _mock = super::mock_guard();
    //given
    let cam = Camera::new("test_camera".to_owned());
    let ctx = OpenQHYCCD_context();
    ctx.expect().times(1).return_const_st(core::ptr::null());
    //when
    let res = cam.open();
    //then
    assert!(res.is_err());
    assert_eq!(res.err().unwrap(), QHYError::Sdk { op: "open_camera" });
}
#[test]
fn open_nulerror() {
    let _mock = super::mock_guard();
    //given
    let cam = Camera::new("test_\0camera".to_owned());
    let ctx = OpenQHYCCD_context();
    ctx.expect().times(0);
    //when
    let res = cam.open();
    //then
    assert!(res.is_err());
    assert_eq!(
        res.err().unwrap(),
        QHYError::InvalidCameraId(std::ffi::CString::new("test_\0camera").unwrap_err())
    );
}

#[test]
fn close_success() {
    let _mock = super::mock_guard();
    //given
    let ctx_open = OpenQHYCCD_context();
    ctx_open.expect().times(1).return_const_st(TEST_HANDLE);
    // One close: the explicit `close()` below takes the handle, so `HandleCell::Drop`
    // is a no-op when `cam` drops. Built manually (not via `new_camera`) so this
    // test owns the sole `CloseQHYCCD` expectation.
    let ctx_close = CloseQHYCCD_context();
    ctx_close.expect().times(1).return_const_st(QHYCCD_SUCCESS);
    let cam = Camera::new("test_camera".to_owned());
    cam.open().unwrap();
    //when
    let res = cam.close();
    //then
    assert!(res.is_ok());
}

#[test]
fn close_already_closed() {
    let _mock = super::mock_guard();
    //given
    let ctx_open = OpenQHYCCD_context();
    ctx_open.expect().times(1).return_const_st(TEST_HANDLE);
    // First close takes the handle; the second close and the drop are both no-ops.
    let ctx_close = CloseQHYCCD_context();
    ctx_close.expect().times(1).return_const_st(QHYCCD_SUCCESS);
    let cam = Camera::new("test_camera".to_owned());
    cam.open().unwrap();
    let _res = cam.close();
    //when
    let res = cam.close();
    //then
    assert!(res.is_ok());
}

#[test]
fn close_fail() {
    let _mock = super::mock_guard();
    //given
    let ctx_open = OpenQHYCCD_context();
    ctx_open.expect().times(1).return_const_st(TEST_HANDLE);
    // A failed close leaves the handle open, so `HandleCell::Drop` retries it when
    // `cam` drops: CloseQHYCCD is called twice (the explicit close + the drop).
    let ctx_close = CloseQHYCCD_context();
    ctx_close.expect().times(2).return_const_st(QHYCCD_ERROR);
    let cam = Camera::new("test_camera".to_owned());
    cam.open().unwrap();
    //when
    let res = cam.close();
    //then
    assert!(res.is_err());
    assert_eq!(res.err().unwrap(), QHYError::Sdk { op: "close_camera" });
}

#[test]
fn bayer_mode_try_from() {
    let _mock = super::mock_guard();
    assert_eq!(BayerMode::try_from(1).unwrap(), BayerMode::GBRG);
    assert_eq!(BayerMode::try_from(2).unwrap(), BayerMode::GRBG);
    assert_eq!(BayerMode::try_from(3).unwrap(), BayerMode::BGGR);
    assert_eq!(BayerMode::try_from(4).unwrap(), BayerMode::RGGB);
    assert!(BayerMode::try_from(0).is_err());
    assert!(BayerMode::try_from(5).is_err());
}

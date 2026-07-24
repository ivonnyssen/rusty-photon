use crate::Result;

use crate::QHYError;

#[cfg(not(feature = "simulation"))]
use crate::backend::{read_lock, QHYCCDHandle};
#[cfg(not(feature = "simulation"))]
use crate::check;
#[cfg(not(feature = "simulation"))]
use crate::sys::{CloseQHYCCD, InitQHYCCD, OpenQHYCCD};

#[cfg(feature = "simulation")]
use crate::CCDChipArea;

use super::Camera;

impl Camera {
    /// Opens a camera with the given id. The SDK automatically finds all connected cameras upon initialization
    /// but does not call open on the cameras. You have to call open on the camera you want to use. Calling open
    /// on a camera that is already open does not do anything.
    /// # Example
    /// ```no_run
    /// use qhyccd_rs::{Sdk,Camera};
    /// let sdk = Sdk::new().expect("SDK::new failed");
    /// let camera = sdk.cameras().last().expect("no camera found");
    /// camera.open().expect("open failed");
    /// ```
    pub fn open(&self) -> Result<()> {
        if self.is_open()? {
            return Ok(());
        }
        #[cfg(not(feature = "simulation"))]
        {
            // read and see if the handle is already Some(_)
            let mut lock = self.handle.write();
            unsafe {
                match std::ffi::CString::new(self.id.clone()) {
                    Ok(c_id) => {
                        let handle = OpenQHYCCD(c_id.as_ptr());
                        if handle.is_null() {
                            let error = QHYError::Sdk { op: "open_camera" };
                            tracing::error!(error = ?error);
                            return Err(error);
                        }
                        *lock = Some(QHYCCDHandle { ptr: handle });
                        Ok(())
                    }
                    Err(error) => {
                        tracing::error!(error = ?error);
                        Err(error.into())
                    }
                }
            }
        }
        #[cfg(feature = "simulation")]
        {
            let mut state = self.state.write();
            state.is_open = true;
            Ok(())
        }
    }

    /// Closes the camera. If you have to call this function, you can then open the camera again by
    /// calling `open`. Calling close on a camera that is not open does not do anything.
    /// # Example
    /// ```no_run
    /// use qhyccd_rs::{Sdk,Camera};
    /// let sdk = Sdk::new().expect("SDK::new failed");
    /// let camera = sdk.cameras().last().expect("no camera found");
    /// camera.open().expect("open failed");
    /// camera.close().expect("close failed");
    /// ```
    pub fn close(&self) -> Result<()> {
        if !self.is_open()? {
            return Ok(());
        }
        #[cfg(not(feature = "simulation"))]
        {
            let mut lock = self.handle.write();

            match *lock {
                Some(handle) => {
                    check(unsafe { CloseQHYCCD(handle.ptr) }, "close_camera")?;
                    lock.take();
                    Ok(())
                }
                None => Ok(()),
            }
        }
        #[cfg(feature = "simulation")]
        {
            let mut state = self.state.write();
            state.is_open = false;
            state.is_initialized = false;
            Ok(())
        }
    }

    /// initializes the camera to a new session - use this to change from LiveMode to SingleFrameMode for instance
    /// # Example
    /// ```no_run
    /// use qhyccd_rs::{Sdk, StreamMode};
    /// let sdk = Sdk::new().expect("SDK::new failed");
    /// let camera = sdk.cameras().last().expect("no camera found");
    /// camera.open().expect("open failed");
    /// camera.set_stream_mode(StreamMode::LiveMode).expect("set_stream_mode failed");
    /// camera.init().expect("init failed");
    /// ```
    pub fn init(&self) -> Result<()> {
        #[cfg(not(feature = "simulation"))]
        {
            let handle = read_lock!(self.handle)?;
            check(unsafe { InitQHYCCD(handle) }, "init_camera")
        }
        #[cfg(feature = "simulation")]
        {
            let mut state = self.state.write();
            if !state.is_open {
                return Err(QHYError::CameraNotOpen);
            }
            state.is_initialized = true;
            // Reset ROI to full frame based on current readout mode
            let (width, height) = state
                .config
                .readout_modes
                .get(state.readout_mode as usize)
                .map(|(_, res)| *res)
                .unwrap_or((
                    state.config.chip_info.image_width,
                    state.config.chip_info.image_height,
                ));
            state.roi = CCDChipArea {
                start_x: 0,
                start_y: 0,
                width,
                height,
            };
            Ok(())
        }
    }

    /// Returns `true` if the camera is open
    /// # Example
    /// ```no_run
    /// use qhyccd_rs::{Sdk,Camera};
    /// let sdk = Sdk::new().expect("SDK::new failed");
    /// let camera = sdk.cameras().last().expect("no camera found"); // this does not open the camera
    /// camera.open().expect("open failed");
    /// let is_open = camera.is_open();
    /// println!("Is camera open: {:?}", is_open);
    /// ```
    pub fn is_open(&self) -> Result<bool> {
        #[cfg(not(feature = "simulation"))]
        {
            let lock = self.handle.read();
            Ok((*lock).is_some())
        }
        #[cfg(feature = "simulation")]
        {
            let state = self.state.read();
            Ok(state.is_open)
        }
    }
}

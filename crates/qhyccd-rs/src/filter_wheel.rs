use crate::Result;
use tracing::error;

use crate::{Camera, ControlType, QHYError};

#[derive(Debug, PartialEq, Clone)]
/// Filter wheels are directly connected to the QHY camera
pub struct FilterWheel {
    camera: Camera,
}

/// Filter wheels are directly connected to the QHY camera and can be controlled through the camera
#[allow(unused_unsafe)]
impl FilterWheel {
    /// Creates a new instance of the filter wheel. The Sdk automatically finds all filter wheels and provides them in its `filter_wheels()` iterator. Creating
    /// a filter wheel manually should only be needed for rare cases.
    ///
    /// The wheel must be built from a camera the SDK handed out: it shares that
    /// camera's backend, so both act on the same device.
    /// # Example
    /// ```no_run
    /// use qhyccd_rs::{FilterWheel, Sdk};
    /// let sdk = Sdk::new().expect("SDK::new failed");
    /// let camera = sdk.cameras().last().expect("no camera found");
    /// let fw = FilterWheel::new(camera.clone());
    /// println!("FilterWheel: {:?}", fw);
    /// ```
    pub fn new(camera: Camera) -> Self {
        Self { camera }
    }

    /// Returns the id of the filter wheel
    /// # Example
    /// ```no_run
    /// use qhyccd_rs::{Sdk,FilterWheel};
    /// let sdk = Sdk::new().expect("SDK::new failed");
    /// let fw = sdk.filter_wheels().last().expect("no filter wheel found");
    /// println!("Filter wheel id: {}", fw.id());
    /// ```
    pub fn id(&self) -> &str {
        self.camera.id()
    }

    /// Opens the filter wheel
    /// # Example
    /// ```no_run
    /// use qhyccd_rs::{Sdk,FilterWheel};
    /// let sdk = Sdk::new().expect("SDK::new failed");
    /// let fw = sdk.filter_wheels().last().expect("no filter wheel found");
    /// fw.open().expect("open failed");
    /// ```
    pub fn open(&self) -> Result<()> {
        self.camera.open()
    }

    /// Returns `true` if the filter wheel is open
    /// # Example
    /// ```no_run
    /// use qhyccd_rs::{Sdk,FilterWheel};
    /// let sdk = Sdk::new().expect("SDK::new failed");
    /// let fw = sdk.filter_wheels().last().expect("no filter wheel found");
    /// fw.open().expect("open failed");
    /// let is_open = fw.is_open();
    /// println!("Is filter wheel open: {:?}", is_open);
    /// ```
    pub fn is_open(&self) -> Result<bool> {
        self.camera.is_open()
    }

    /// Returns `true` if the filter wheel is plugged into the camera
    /// # Example
    /// ```no_run
    /// use qhyccd_rs::{Sdk,FilterWheel};
    /// let sdk = Sdk::new().expect("SDK::new failed");
    /// let fw = sdk.filter_wheels().last().expect("no filter wheel found");
    /// fw.open().expect("open failed");
    /// let is_cfw_plugged_in = fw.is_cfw_plugged_in().expect("is_cfw_plugged_in failed");
    /// println!("Is filter wheel plugged in: {}", is_cfw_plugged_in);
    /// ```
    pub fn is_cfw_plugged_in(&self) -> Result<bool> {
        self.camera.is_cfw_plugged_in()
    }

    /// Closes the filter wheel
    /// # Example
    /// ```no_run
    /// use qhyccd_rs::{Sdk,FilterWheel};
    /// let sdk = Sdk::new().expect("SDK::new failed");
    /// let fw = sdk.filter_wheels().last().expect("no filter wheel found");
    /// fw.open().expect("open failed");
    /// fw.close().expect("close failed");
    /// ```
    pub fn close(&self) -> Result<()> {
        self.camera.close()
    }

    /// Returns the number of filters in the filter wheel
    /// # Example
    /// ```no_run
    /// use qhyccd_rs::{Sdk,FilterWheel};
    /// let sdk = Sdk::new().expect("SDK::new failed");
    /// let fw = sdk.filter_wheels().last().expect("no filter wheel found");
    /// fw.open().expect("open failed");
    /// let number_of_filters = fw.get_number_of_filters().expect("get_number_of_filters failed");
    /// println!("Number of filters: {}", number_of_filters);
    /// ```
    pub fn get_number_of_filters(&self) -> Result<u32> {
        match self.camera.is_control_available(ControlType::CfwSlotsNum) {
            Some(_) => self.camera.cfw_slot_count().map_err(|e| {
                error!(?e, "could not get number of filters from camera");
                e
            }),
            None => {
                tracing::debug!("I'm a filter wheel without filters. :(");
                Err(QHYError::Sdk {
                    op: "get_number_of_filters",
                })
            }
        }
    }

    /// Returns the current filter wheel position
    /// # Example
    /// ```no_run
    /// use qhyccd_rs::{Sdk,FilterWheel};
    /// let sdk = Sdk::new().expect("SDK::new failed");
    /// let fw = sdk.filter_wheels().last().expect("no filter wheel found");
    /// fw.open().expect("open failed");
    /// let current_position = fw.get_fw_position().expect("get_fw_position failed");
    /// println!("Current position: {}", current_position);
    /// ```
    pub fn get_fw_position(&self) -> Result<u32> {
        match self.camera.is_control_available(ControlType::CfwPort) {
            // `cfw_position` decodes the SDK's ASCII position offset.
            Some(_) => self.camera.cfw_position().map_err(|error| {
                tracing::error!(error = ?error);
                error
            }),
            None => {
                tracing::debug!("No filter wheel plugged in.");
                Err(QHYError::Sdk {
                    op: "get_fw_position",
                })
            }
        }
    }

    /// Sets the current filter wheel position
    /// # Example
    /// ```no_run
    /// use qhyccd_rs::{Sdk,FilterWheel};
    /// let sdk = Sdk::new().expect("SDK::new failed");
    /// let fw = sdk.filter_wheels().last().expect("no filter wheel found");
    /// fw.open().expect("open failed");
    /// fw.set_fw_position(1).expect("set_fw_position failed");
    /// ```
    pub fn set_fw_position(&self, position: u32) -> Result<()> {
        match self.camera.is_control_available(ControlType::CfwPort) {
            // `set_cfw_position` applies the SDK's ASCII position offset.
            Some(_) => self.camera.set_cfw_position(position).map_err(|_| {
                let error = QHYError::Sdk {
                    op: "set_fw_position",
                };
                tracing::error!(error = ?error);
                error
            }),
            None => {
                tracing::debug!("No filter wheel plugged in.");
                Err(QHYError::Sdk {
                    op: "set_fw_position",
                })
            }
        }
    }
}

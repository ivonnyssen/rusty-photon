//struct to describe fits file
#[derive(Debug)]
pub struct FitsFile {
    pub file_name: String,
    pub file_size: u32,
    pub file_type: String,
    pub file_data: Vec<u8>,
}

//function to read fits file
pub fn read_fits(file: &std::fs::File) -> FitsFile {
    println!("Reading fits file {file:?}");
    FitsFile {
        file_name: String::from("test.fits"),
        file_size: 0,
        file_type: String::from(""),
        file_data: Vec::new(),
    }
}
//function to write fits file
pub fn write_fits(file: &std::fs::File, data: &self::FitsFile) -> Result<(), std::io::Error> {
    println!("Writing fits file {file:?} : {data:?}");
    Ok(())
}

use rusty_photon::fits;
use std::env;
use std::fs::File;

fn main() -> Result<(), std::io::Error> {
    let args: Vec<String> = env::args().collect();
    let file_name = &args[1];

    //create a fits file
    let fits_file = fits::FitsFile {
        file_name: file_name.to_string(),
        file_size: 0,
        file_type: String::from(""),
        file_data: Vec::new(),
    };

    let file = File::open(file_name)?;
    //read the fits file
    fits::read_fits(&file);
    //write the fits file
    fits::write_fits(&file, &fits_file)
}

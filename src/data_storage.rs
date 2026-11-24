use std::{fs::File, io, path::Path};

#[derive(Debug)]
pub(crate) enum ImageError {
    MissingPath,
    MissingFile,
    Io(io::Error),
}

pub(crate) fn open_image(path: Option<&Path>) -> Result<File, ImageError> {
    let path = path
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or(ImageError::MissingPath)?;
    let file = File::open(path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            ImageError::MissingFile
        } else {
            ImageError::Io(error)
        }
    })?;
    if !file.metadata().map_err(ImageError::Io)?.is_file() {
        return Err(ImageError::MissingFile);
    }
    Ok(file)
}

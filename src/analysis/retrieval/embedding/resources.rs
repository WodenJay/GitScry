use crate::Error;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{fs::File, io::Read, path::Path};

#[derive(Deserialize)]
struct Manifest {
    files: Vec<Resource>,
}
#[derive(Deserialize)]
struct Resource {
    name: String,
    size: u64,
    sha256: String,
}

pub(crate) fn verify(directory: &Path) -> Result<(), Error> {
    let manifest: Manifest = serde_json::from_str(include_str!("resources.json"))
        .map_err(|error| Error::Resource(error.to_string()))?;
    for resource in manifest.files {
        let verify = || -> Result<(), Box<dyn std::error::Error>> {
            let mut file = File::open(directory.join(&resource.name))?;
            if file.metadata()?.len() != resource.size {
                return Err("size mismatch".into());
            }
            let mut hash = Sha256::new();
            let mut buffer = [0u8; 65536];
            loop {
                let size = file.read(&mut buffer)?;
                if size == 0 {
                    break;
                }
                hash.update(&buffer[..size]);
            }
            if format!("{:x}", hash.finalize()) != resource.sha256 {
                return Err("SHA-256 mismatch".into());
            }
            Ok(())
        };
        verify().map_err(|error| Error::Resource(format!("{}: {error}", resource.name)))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vendored_tokenizer_is_the_pinned_resource() {
        let manifest: Manifest = serde_json::from_str(include_str!("resources.json")).unwrap();
        let resource = manifest
            .files
            .iter()
            .find(|file| file.name == "tokenizer.json")
            .unwrap();
        let bytes = include_bytes!("fixtures/tokenizer.json");
        assert_eq!(bytes.len() as u64, resource.size);
        assert_eq!(format!("{:x}", Sha256::digest(bytes)), resource.sha256);
    }

    #[test]
    fn missing_resources_fail_offline() {
        let directory =
            std::env::temp_dir().join(format!("gitscry-no-model-{}", std::process::id()));
        assert!(!directory.exists());
        let error = verify(&directory).unwrap_err().to_string();
        assert!(error.contains("model.onnx"));
        assert!(!directory.exists());
    }
}

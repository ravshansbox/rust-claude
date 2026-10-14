use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{Session, hex, sessions_directory};
use crate::{config, images::Image};

impl Session {
    fn images_directory(&self) -> Result<PathBuf> {
        Ok(sessions_directory()?.join(&self.id))
    }

    pub fn save_image(&self, image: &Image) -> Result<Value> {
        let directory = self.images_directory()?;
        let hash = hex(&Sha256::digest(&image.data)[..8]);
        let file = format!("{hash}.{}", image.extension());
        let path = directory.join(&file);
        // Images are written whole through a temporary file, so a file of
        // another length is what an older version left cut short.
        let saved = std::fs::metadata(&path)
            .is_ok_and(|metadata| usize::try_from(metadata.len()) == Ok(image.data.len()));
        if !saved {
            config::write_private_file(&path, &image.data)?;
        }
        Ok(json!({ "type": "image", "file": file, "media_type": image.media_type }))
    }

    pub fn inline_images(&self, messages: &mut [Value]) -> Result<()> {
        inline_images(&self.images_directory()?, messages)
    }
}

pub(super) fn inline_images(directory: &Path, messages: &mut [Value]) -> Result<()> {
    let blocks = messages
        .iter_mut()
        .filter_map(|message| message["content"].as_array_mut())
        .flatten()
        .filter(|block| block["type"] == "image");
    for block in blocks {
        let Some(object) = block.as_object_mut() else {
            continue;
        };
        let Some(file) = object.remove("file") else {
            continue;
        };
        let file = file.as_str().unwrap_or_default();
        let data = std::fs::read(directory.join(file))
            .with_context(|| format!("failed to read image {file}"))?;
        let media_type = object.remove("media_type").unwrap_or_default();
        object.insert(
            "source".into(),
            json!({ "type": "base64", "media_type": media_type, "data": STANDARD.encode(data) }),
        );
    }
    Ok(())
}

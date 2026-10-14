use std::{io::Cursor, path::Path};

use anyhow::{Context, Result};
use serde::Deserialize;

use super::{ASSET, PACKAGE, Updater};

#[derive(Deserialize)]
pub(super) struct Release {
    pub(super) tag_name: String,
    #[serde(default)]
    pub(super) assets: Vec<Asset>,
}

#[derive(Deserialize)]
pub(super) struct Asset {
    pub(super) name: String,
    pub(super) browser_download_url: String,
}

impl Updater {
    fn get(&self, url: &str) -> reqwest::RequestBuilder {
        self.http
            .get(url)
            .header(reqwest::header::USER_AGENT, PACKAGE)
    }

    pub(super) async fn latest_release(&self) -> Result<Release> {
        Ok(self
            .get(&self.releases_url)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    async fn fetch(&self, url: &str) -> Result<Vec<u8>> {
        Ok(self
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?
            .into())
    }

    /// Downloads the zip and unpacks it, once its signature shows it was
    /// made with the release key.
    pub(super) async fn download(
        &self,
        url: &str,
        signature_url: &str,
        version: &str,
        folder: &Path,
    ) -> Result<()> {
        let bytes = self.fetch(url).await?;
        let signature = self.fetch(signature_url).await?;
        let signed_as = minisign_verify::PublicKey::from_base64(self.public_key)
            .and_then(|key| {
                let signature =
                    minisign_verify::Signature::decode(&String::from_utf8_lossy(&signature))?;
                key.verify(&bytes, &signature, false)?;
                Ok(signature.trusted_comment().to_string())
            })
            .map_err(|_| anyhow::anyhow!("the signature of {ASSET} is not valid"))?;
        if signed_as.strip_prefix("rust-coding-agent ") != Some(version) {
            anyhow::bail!("{ASSET} is signed as {signed_as}, not {version}");
        }
        let target = folder.to_path_buf();
        tokio::task::spawn_blocking(move || {
            zip::ZipArchive::new(Cursor::new(bytes))?.extract(target)
        })
        .await?
        .with_context(|| format!("unpacking {ASSET}"))
    }
}

use std::{path::Path, time::Instant};

use anyhow::{Context, Result};

use super::{ASSET, Progress, Updater, download::Release};

const SIGNATURE_ASSET: &str = "rust-coding-agent.zip.minisig";

impl Updater {
    pub(super) async fn install(
        &self,
        release: &Release,
        version: &str,
        cargo: &Path,
        folder: &Path,
        report: &impl Fn(Progress),
    ) -> Result<(), Progress> {
        let asset_url = |name: &str| {
            release
                .assets
                .iter()
                .find(|asset| asset.name == name)
                .map(|asset| asset.browser_download_url.clone())
                .with_context(|| format!("the release has no {name}"))
                .map_err(|error| Progress::Failed(format!("{error:#}")))
        };
        let url = asset_url(ASSET)?;
        let signature_url = asset_url(SIGNATURE_ASSET)?;
        report(Progress::Downloading);
        let started = Instant::now();
        self.download(&url, &signature_url, version, folder)
            .await
            .map_err(|error| Progress::DownloadFailed(format!("{error:#}"), started.elapsed()))?;
        report(Progress::Downloaded(started.elapsed()));
        report(Progress::Building);
        let started = Instant::now();
        self.build(cargo, folder)
            .await
            .map_err(|error| Progress::BuildFailed(format!("{error:#}"), started.elapsed()))?;
        report(Progress::Built(started.elapsed()));
        Ok(())
    }
}

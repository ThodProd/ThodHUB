use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use chrono::{DateTime, Utc};
use futures_util::stream::{self, StreamExt};
use quick_xml::events::Event;
use reqwest::StatusCode;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

use crate::contract;
use crate::models::{Catalog, CatalogApp, HubReleaseInfo, PackageRelease};
use crate::store::HubStore;
use crate::util;

pub struct GitHubCatalog {
    client: reqwest::Client,
    download_client: reqwest::Client,
}

#[derive(Clone, Debug)]
struct AtomRelease {
    tag: String,
    version: String,
    release_url: String,
    published_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize)]
struct PrivateRepo {
    private: bool,
}

impl GitHubCatalog {
    pub fn new() -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .user_agent("ThodHUB/0.2.3")
            .build()?;
        // GitHub's HTTP/2 + compressed asset responses can stall or throttle
        // multi-hundred-megabyte downloads. HTTP/1.1 without transparent
        // decompression matches the shipping .NET hub and downloads at full speed.
        let download_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15 * 60))
            .user_agent("ThodHUB/0.2.3")
            .http1_only()
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .build()?;
        Ok(Self {
            client,
            download_client,
        })
    }

    pub fn load_cached(&self, store: &HubStore, bundled_path: &Path) -> Result<Catalog> {
        let cache = store.cache_root().join("catalog.json");
        if cache.exists() {
            if let Ok(catalog) = util::json_read::<Catalog>(&cache) {
                if contract::validate_catalog(&catalog).is_ok() {
                    return Ok(catalog);
                }
            }
        }
        let catalog = util::json_read::<Catalog>(bundled_path)?;
        contract::validate_catalog(&catalog)?;
        Ok(catalog)
    }

    pub async fn refresh(&self, store: &HubStore, previous: &Catalog) -> Result<Catalog> {
        let settings = store.settings();
        let repository = settings.catalog_repository;
        contract::repository(&repository)?;
        let ref_name = settings
            .catalog_ref
            .split('/')
            .map(util::escape_url_component)
            .collect::<Vec<_>>()
            .join("/");
        let url = format!("https://raw.githubusercontent.com/{repository}/{ref_name}/catalog.json");
        let text = self
            .get_text_optional(&url, 2_000_000)
            .await?
            .ok_or_else(|| anyhow!("Каталог не найден."))?;
        let mut catalog = util::json_read_text::<Catalog>(&text)?;
        contract::validate_catalog(&catalog)?;

        let installed = store.settings().installed;
        let retired: Vec<crate::models::CatalogApp> = previous
            .apps
            .iter()
            .filter(|a| {
                installed.iter().any(|i| i.id == a.id)
                    && !catalog.apps.iter().any(|n| n.id == a.id)
            })
            .cloned()
            .map(|mut old| {
                old.release = None;
                old.release_error =
                    Some("Программа больше не входит в удалённый каталог.".into());
                old
            })
            .collect();
        catalog.apps.extend(retired);

        let client = self.client.clone();
        let catalog_repository = repository.clone();
        let results: Vec<(usize, Result<Option<PackageRelease>>)> = stream::iter(
            catalog.apps.iter().cloned().enumerate(),
        )
        .map(|(index, app)| {
            let client = client.clone();
            let repository = catalog_repository.clone();
            async move {
                let result = find_latest_release(&client, &repository, &app.repository).await;
                (index, result)
            }
        })
        .buffer_unordered(4)
        .collect()
        .await;

        for (index, result) in results {
            let app = &mut catalog.apps[index];
            match result {
                Ok(Some(release)) => app.release = Some(release),
                Ok(None) => {
                    if let Some(old) = previous
                        .apps
                        .iter()
                        .find(|x| x.id == app.id && x.repository == app.repository)
                    {
                        app.release = old.release.clone();
                    }
                    app.release_error = None;
                }
                Err(error) => {
                    if let Some(old) = previous
                        .apps
                        .iter()
                        .find(|x| x.id == app.id && x.repository == app.repository)
                    {
                        app.release = old.release.clone();
                    }
                    app.release_error = Some(format!("Не удалось проверить выпуск. {error}"));
                }
            }
        }

        let text = util::json_write(&catalog)?;
        util::write_atomic(&store.cache_root().join("catalog.json"), &text)?;
        Ok(catalog)
    }

    async fn find_latest_release(
        &self,
        repository: &str,
        ct: &CancellationToken,
    ) -> Result<Option<PackageRelease>> {
        for release in self.read_releases(repository, ct).await? {
            let Some(mut descriptor) = self
                .try_read_descriptor(repository, &release.tag, ct)
                .await?
            else {
                continue;
            };
            if descriptor.version != release.version {
                continue;
            }
            contract::release(&descriptor, repository)?;
            descriptor.published_at = release.published_at;
            return Ok(Some(descriptor));
        }
        Ok(None)
    }

    async fn read_releases(
        &self,
        repository: &str,
        ct: &CancellationToken,
    ) -> Result<Vec<AtomRelease>> {
        let url = format!("https://github.com/{repository}/releases.atom");
        let Some(xml) = self.get_text_optional_cancellable(&url, 2_000_000, ct).await? else {
            return Ok(Vec::new());
        };
        parse_atom_releases(&xml, repository)
    }

    async fn try_read_descriptor(
        &self,
        repository: &str,
        tag: &str,
        ct: &CancellationToken,
    ) -> Result<Option<PackageRelease>> {
        let url = format!(
            "https://github.com/{repository}/releases/download/{}/thodhub-release.json",
            util::escape_url_component(tag)
        );
        let Some(text) = self.get_text_optional_cancellable(&url, 100_000, ct).await? else {
            return Ok(None);
        };
        Ok(Some(util::json_read_text::<PackageRelease>(&text)?))
    }

    pub async fn check_hub_update(&self, current: &str) -> Result<Option<HubReleaseInfo>> {
        const REPOSITORY: &str = "ThodProd/ThodHUB";
        contract::version(current)?;
        let ct = CancellationToken::new();
        let mut best: Option<HubReleaseInfo> = None;
        for release in self.read_releases(REPOSITORY, &ct).await? {
            if !contract::is_newer(&release.version, current)? {
                continue;
            }
            if let Some(best) = &best {
                if !contract::is_newer(&release.version, &best.version)? {
                    continue;
                }
            }
            let Some(descriptor) = self
                .try_read_descriptor(REPOSITORY, &release.tag, &ct)
                .await?
            else {
                continue;
            };
            if descriptor.version != release.version {
                continue;
            }
            contract::release(&descriptor, REPOSITORY)?;
            if !is_repo_url(&descriptor.package_url, REPOSITORY, "releases/download/") {
                continue;
            }
            best = Some(HubReleaseInfo {
                version: release.version,
                notes: descriptor.notes,
                size: descriptor.size,
                sha256: descriptor.sha256,
                download_url: Some(descriptor.package_url),
                release_url: release.release_url,
                published_at: release.published_at,
            });
        }
        Ok(best)
    }

    pub async fn download<F>(
        &self,
        store: &HubStore,
        app: &CatalogApp,
        mut progress: F,
        ct: &CancellationToken,
    ) -> Result<PathBuf>
    where
        F: FnMut(f64),
    {
        let release = app
            .release
            .as_ref()
            .ok_or_else(|| anyhow!("Нет готового выпуска."))?;
        contract::release(release, &app.repository)?;
        let target = store
            .cache_root()
            .join(format!("{}.zip", util::random_token()));
        let result = self
            .download_to(release, &target, &mut progress, ct)
            .await;
        if result.is_err() {
            let _ = tokio::fs::remove_file(&target).await;
        }
        result.map(|_| target)
    }

    async fn download_to<F>(
        &self,
        release: &PackageRelease,
        target: &Path,
        progress: &mut F,
        ct: &CancellationToken,
    ) -> Result<()>
    where
        F: FnMut(f64),
    {
        let timeout = tokio::time::timeout(Duration::from_secs(15 * 60), async {
            let mut response = self
                .download_client
                .get(&release.package_url)
                .send()
                .await?
                .error_for_status()?;
            if let Some(length) = response.content_length() {
                if length as i64 != release.size {
                    return Err(anyhow!("Размер выпуска изменился."));
                }
            }
            let mut file = tokio::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(target)
                .await?;
            let mut received: i64 = 0;
            let mut last_progress = Instant::now();
            loop {
                let chunk = tokio::select! {
                    _ = ct.cancelled() => return Err(anyhow!("Действие отменено.")),
                    chunk = response.chunk() => chunk?,
                };
                let Some(chunk) = chunk else {
                    break;
                };
                received += chunk.len() as i64;
                if received > release.size {
                    return Err(anyhow!("Пакет превышает заявленный размер."));
                }
                file.write_all(&chunk).await?;
                if received == release.size || last_progress.elapsed() >= Duration::from_millis(100)
                {
                    progress(received as f64 / release.size as f64);
                    last_progress = Instant::now();
                }
            }
            file.flush().await?;
            if received != release.size {
                return Err(anyhow!("Загрузка прервалась до конца файла."));
            }
            Ok(())
        })
        .await;
        match timeout {
            Ok(result) => result,
            Err(_) => Err(anyhow!("Загрузка не завершилась за 15 минут.")),
        }
    }

    pub async fn download_hub_update<F>(
        &self,
        store: &HubStore,
        update: &HubReleaseInfo,
        progress: F,
        ct: &CancellationToken,
    ) -> Result<PathBuf>
    where
        F: FnMut(f64),
    {
        let download_url = update
            .download_url
            .clone()
            .ok_or_else(|| anyhow!("Нет готового пакета обновления хаба."))?;
        if update.sha256.len() != 64 {
            return Err(anyhow!("Нет готового пакета обновления хаба."));
        }
        let app = CatalogApp {
            id: "thodhub".into(),
            name: "ThodHUB".into(),
            description: "Обновление хаба".into(),
            repository: "ThodProd/ThodHUB".into(),
            release: Some(PackageRelease {
                version: update.version.clone(),
                package_url: download_url,
                sha256: update.sha256.clone(),
                size: update.size,
                notes: update.notes.clone(),
                published_at: update.published_at,
                ..Default::default()
            }),
            ..Default::default()
        };
        let path = self.download(store, &app, progress, ct).await?;
        let hash = util::sha256_file_async(&path).await?;
        if !hash.eq_ignore_ascii_case(&update.sha256) {
            let _ = tokio::fs::remove_file(&path).await;
            return Err(anyhow!("SHA-256 обновления не совпадает."));
        }
        Ok(path)
    }

    async fn get_text_optional(&self, url: &str, limit: usize) -> Result<Option<String>> {
        self.get_text_optional_cancellable(url, limit, &CancellationToken::new())
            .await
    }

    async fn get_text_optional_cancellable(
        &self,
        url: &str,
        limit: usize,
        ct: &CancellationToken,
    ) -> Result<Option<String>> {
        let response = tokio::select! {
            _ = ct.cancelled() => return Err(anyhow!("Действие отменено.")),
            response = self.client.get(url).send() => response?,
        };
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let mut response = response.error_for_status()?;
        if let Some(length) = response.content_length() {
            if length as usize > limit {
                return Err(anyhow!("Ответ сервера слишком большой."));
            }
        }
        let mut buffer = Vec::new();
        loop {
            let chunk = tokio::select! {
                _ = ct.cancelled() => return Err(anyhow!("Действие отменено.")),
                chunk = response.chunk() => chunk?,
            };
            let Some(chunk) = chunk else {
                break;
            };
            if buffer.len() + chunk.len() > limit {
                return Err(anyhow!("Ответ сервера слишком большой."));
            }
            buffer.extend_from_slice(&chunk);
        }
        Ok(Some(String::from_utf8_lossy(&buffer).to_string()))
    }
}

async fn find_latest_release(
    client: &reqwest::Client,
    catalog_repository: &str,
    repository: &str,
) -> Result<Option<PackageRelease>> {
    contract::repository(repository)?;
    let _ = catalog_repository;
    let url = format!("https://github.com/{repository}/releases.atom");
    let Some(xml) = get_text(client, &url, 2_000_000).await? else {
        return Ok(None);
    };
    for release in parse_atom_releases(&xml, repository)? {
        let descriptor_url = format!(
            "https://github.com/{repository}/releases/download/{}/thodhub-release.json",
            util::escape_url_component(&release.tag)
        );
        let Some(text) = get_text(client, &descriptor_url, 100_000).await? else {
            continue;
        };
        let Ok(mut descriptor) = util::json_read_text::<PackageRelease>(&text) else {
            continue;
        };
        if descriptor.version != release.version {
            continue;
        }
        contract::release(&descriptor, repository)?;
        descriptor.published_at = release.published_at;
        return Ok(Some(descriptor));
    }
    Ok(None)
}

async fn get_text(client: &reqwest::Client, url: &str, limit: usize) -> Result<Option<String>> {
    let response = client.get(url).send().await?;
    if response.status() == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let mut response = response.error_for_status()?;
    if let Some(length) = response.content_length() {
        if length as usize > limit {
            return Err(anyhow!("Ответ сервера слишком большой."));
        }
    }
    let mut buffer = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if buffer.len() + chunk.len() > limit {
            return Err(anyhow!("Ответ сервера слишком большой."));
        }
        buffer.extend_from_slice(&chunk);
    }
    Ok(Some(String::from_utf8_lossy(&buffer).to_string()))
}

fn parse_atom_releases(xml: &str, repository: &str) -> Result<Vec<AtomRelease>> {
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut releases = Vec::new();
    let mut in_entry = false;
    let mut tag = String::new();
    let mut link: Option<String> = None;
    let mut published: Option<String> = None;
    let mut updated: Option<String> = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) => {
                let name = event.local_name().as_ref().to_string();
                if name == "entry" {
                    in_entry = true;
                    link = None;
                    published = None;
                    updated = None;
                } else if in_entry && name == "link" {
                    read_link(&event, &mut link)?;
                }
                tag = name;
            }
            Ok(Event::Empty(event)) => {
                let name = event.local_name().as_ref().to_string();
                if in_entry && name == "link" {
                    read_link(&event, &mut link)?;
                }
            }
            Ok(Event::Text(event)) => {
                if in_entry {
                    let text = event
                        .xml_content(quick_xml::XmlVersion::Explicit1_0)
                        .to_string();
                    match tag.as_str() {
                        "published" => published = Some(text),
                        "updated" => updated = Some(text),
                        _ => {}
                    }
                }
            }
            Ok(Event::End(event)) => {
                let name = event.local_name().as_ref().to_string();
                if name == "entry" {
                    in_entry = false;
                    if let Some(link) = link.take() {
                        if let Some(tag) = tag_from_repo_url(&link, repository) {
                            if let Some(version) = contract::version_from_tag(&tag) {
                                let date_text = published.as_deref().or(updated.as_deref());
                                releases.push(AtomRelease {
                                    tag,
                                    version,
                                    release_url: link,
                                    published_at: date_text.and_then(parse_date),
                                });
                            }
                        }
                    }
                }
                tag = String::new();
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(error) => return Err(anyhow!("Некорректный Atom-канал GitHub: {error}")),
        }
    }
    Ok(releases)
}

fn read_link(event: &quick_xml::events::BytesStart<'_>, link: &mut Option<String>) -> Result<()> {
    let mut rel: Option<String> = None;
    let mut href: Option<String> = None;
    for attribute in event.attributes().flatten() {
        let key = attribute.key.local_name().as_ref().to_string();
        let value = attribute
            .normalized_value(quick_xml::XmlVersion::Explicit1_0)
            .unwrap_or_default()
            .to_string();
        match key.as_str() {
            "rel" => rel = Some(value),
            "href" => href = Some(value),
            _ => {}
        }
    }
    if rel.as_deref() == Some("alternate") {
        *link = href;
    }
    Ok(())
}

fn parse_date(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|d| d.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%dT%H:%M:%SZ")
                .ok()
                .map(|d| d.and_utc())
        })
}

fn tag_from_repo_url(value: &str, repository: &str) -> Option<String> {
    let url = url::Url::parse(value).ok()?;
    let default_port = url.port().is_none() || url.port() == Some(443);
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || !default_port
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    let marker = format!("/{repository}/releases/tag/");
    let index = url.path().find(&marker)?;
    let tag = &url.path()[index + marker.len()..];
    Some(percent_encoding::percent_decode_str(tag).decode_utf8_lossy().to_string())
}

fn is_repo_url(value: &str, repository: &str, path: &str) -> bool {
    let Ok(url) = url::Url::parse(value) else {
        return false;
    };
    let default_port = url.port().is_none() || url.port() == Some(443);
    url.scheme() == "https"
        && url.host_str() == Some("github.com")
        && default_port
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && url.path().starts_with(&format!("/{repository}/{path}"))
}

/// Small helper used by the repository checks in this module and by GitHubBackups.
pub fn is_private_repository(value: &serde_json::Value) -> bool {
    value
        .get("private")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

pub fn describe_private_repo(text: &str) -> Result<PrivateRepo> {
    Ok(serde_json::from_str(text)?)
}

pub fn hash_bytes(bytes: &[u8]) -> String {
    hex::encode_upper(Sha256::digest(bytes))
}

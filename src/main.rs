#[macro_use]
extern crate slog_scope;

use anyhow::{bail, format_err, Result};
use clap::Parser;
use serde::{Deserialize, Serialize};
use slog::{o, Drain};
use std::os::unix::prelude::PermissionsExt;

#[derive(Debug, Deserialize)]
struct AssetFileRegex {
    #[serde(with = "serde_regex")]
    pattern: regex::Regex,
}

#[derive(Debug, Deserialize)]
struct ReleaseDateRange {
    min: Option<chrono::DateTime<chrono::Local>>,
    max: Option<chrono::DateTime<chrono::Local>>,
}

#[derive(Debug, Deserialize)]
struct ReleaseDateWindow {
    #[serde(with = "humantime_serde")]
    min_from_now: Option<std::time::Duration>,
    #[serde(with = "humantime_serde")]
    max_from_now: Option<std::time::Duration>,
}

#[derive(Debug, Deserialize)]
enum ReleaseFilter {
    AllowAll,
    DateRange(ReleaseDateRange),
    DateWindow(ReleaseDateWindow),
    FixedList(Vec<String>),
}

#[derive(Debug, Deserialize)]
enum AssetFilter {
    AllowAll,
    FileRegex(AssetFileRegex),
}

#[derive(Debug, Deserialize)]
struct Repository {
    path: String,
    release_filter: ReleaseFilter,
    asset_filter: AssetFilter,
}

#[derive(Debug, Deserialize)]
struct Storage {
    path: std::path::PathBuf,
}

#[derive(Debug, Deserialize)]
struct Config {
    storage: std::path::PathBuf,
    repositories: Vec<Repository>,
}

#[derive(Debug, Deserialize, Serialize)]
struct GithubAsset {
    browser_download_url: String,
    name: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct GithubRelease {
    tag_name: String,
    published_at: chrono::DateTime<chrono::Local>,
    assets: Vec<GithubAsset>,
    tarball_url: String,
    zipball_url: String,
}

#[derive(Parser, Debug)]
struct CmdListReleases {
    repository: String,
}

#[derive(Parser, Debug)]
struct CmdMirror {
    config_path: std::path::PathBuf,
}

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
enum Application {
    ListReleases(CmdListReleases),
    Mirror(CmdMirror),
}

impl Config {
    pub fn read(file: &std::path::Path) -> Result<Self> {
        let config = std::fs::read_to_string(file)
            .map_err(|err| format_err!("Failed to load config file {:?}: {}", file, err))?;
        let config: Self = serde_yaml::from_str(&config)
            .map_err(|err| format_err!("Failed to parse config file {:?}: {}", file, err))?;
        Ok(config)
    }
}

impl ReleaseFilter {
    pub fn is_required(&self, release: &GithubRelease) -> bool {
        match self {
            ReleaseFilter::AllowAll => true,
            ReleaseFilter::DateRange(v) => {
                if let Some(min) = v.min {
                    if min > release.published_at {
                        return false;
                    }
                }
                if let Some(max) = v.max {
                    if max < release.published_at {
                        return false;
                    }
                }

                true
            }
            ReleaseFilter::DateWindow(v) => {
                if let Some(min) = v.min_from_now {
                    if chrono::Utc::now() - chrono::Duration::from_std(min).unwrap()
                        > release.published_at
                    {
                        return false;
                    }
                }
                if let Some(max) = v.max_from_now {
                    if chrono::Utc::now() - chrono::Duration::from_std(max).unwrap()
                        < release.published_at
                    {
                        return false;
                    }
                }

                true
            }
            ReleaseFilter::FixedList(list) => list.contains(&release.tag_name),
        }
    }
}

impl AssetFilter {
    pub fn is_required(&self, asset: &GithubAsset) -> bool {
        match self {
            AssetFilter::AllowAll => true,
            AssetFilter::FileRegex(c) => c.pattern.is_match(&asset.name),
        }
    }
}

fn list_releases(repository: &str) -> Result<Vec<GithubRelease>> {
    let http_client = reqwest::blocking::ClientBuilder::new()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("github-mirror-releases")
        .build()?;

    let mut result_list = Vec::new();

    let url = format!("https://api.github.com/repos/{}/releases", repository);
    for page in 1..60 {
        let mut url = url::Url::parse(&url)?;
        url.query_pairs_mut().append_pair("per_page", "30");
        url.query_pairs_mut()
            .append_pair("page", &format!("{}", page));

        info!("Querying {:?}", &url.to_string());

        let res = http_client.get(url).send()?.text()?;

        let mut data = serde_json::de::from_str::<Vec<GithubRelease>>(&res)?;

        if data.is_empty() {
            break;
        }

        for release in &mut data {
            release.assets.push(GithubAsset {
                browser_download_url: release.tarball_url.clone(),
                name: format!("{}.tar.gz", release.tag_name),
            });
            release.assets.push(GithubAsset {
                browser_download_url: release.zipball_url.clone(),
                name: format!("{}.zip", release.tag_name),
            });
        }

        result_list.append(&mut data);
    }

    Ok(result_list)
}

fn cmd_list_releases(args: &CmdListReleases) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string(&list_releases(&args.repository)?).unwrap()
    );

    Ok(())
}

impl GithubAsset {
    pub fn download(
        &self,
        storage: &Storage,
        repository: &str,
        release: &GithubRelease,
    ) -> Result<()> {
        info!("Downloading {}", &self.browser_download_url);

        let destination_directory = storage
            .path
            .join(std::path::PathBuf::from(repository))
            .join(&release.tag_name);

        let destination_file_name = destination_directory.join(&self.name);
        if destination_file_name.exists() {
            info!("Asset already downloaded, skipping");
            return Ok(());
        }

        let http_client = reqwest::blocking::ClientBuilder::new()
            .timeout(std::time::Duration::from_secs(60))
            .user_agent("github-mirror-releases")
            .build()?;

        let mut tmpfile = tempfile::NamedTempFile::new_in(&storage.path)?;
        let mut resp = http_client.get(&self.browser_download_url).send()?;
        let _ = resp.copy_to(&mut tmpfile)?;

        std::fs::create_dir_all(&destination_directory)?;

        let _file = tmpfile.persist(&destination_file_name)?;
        std::fs::set_permissions(
            destination_file_name,
            std::fs::Permissions::from_mode(0o644),
        )?;

        Ok(())
    }
}

impl Storage {
    pub fn init(path: &std::path::PathBuf) -> Result<Self> {
        if let Err(err) = std::fs::create_dir_all(path) {
            crit!("Failed to create storage directory {:?}: {}", &path, err);
            bail!("Failed to create storage directory")
        }

        for path in std::fs::read_dir(path)? {
            let path = path?.path();
            let file_name = match path.file_name() {
                Some(v) => v,
                None => continue,
            };
            if file_name.to_string_lossy().starts_with(".tmp") && path.is_file() {
                std::fs::remove_file(path)?
            }
        }

        Ok(Storage {
            path: path.to_path_buf(),
        })
    }
}

impl GithubRelease {
    pub fn mirror(&self, config: &Config, storage: &Storage, repository: &Repository) {
        info!("Processing release {:?}", &self.tag_name);
        if self.tag_name.contains('/') {
            warn!(
                "Release {:?} contains slash which is prohibited. Skipping.",
                self.tag_name
            );
            return;
        }

        let is_required = repository.release_filter.is_required(self);

        let release_directory = config
            .storage
            .join(std::path::PathBuf::from(&repository.path))
            .join(&self.tag_name);

        if !is_required {
            info!("Skipping release {:?} by filter", self.tag_name);
            if release_directory.exists() {
                info!("Cleaning up unwanted release {:?}", self.tag_name);
                if let Err(err) = std::fs::remove_dir_all(&release_directory) {
                    warn!("Failed to remove {:?}: {}", release_directory, err)
                }
            }

            return;
        }

        for asset in &self.assets {
            let is_required = repository.asset_filter.is_required(asset);
            if !is_required {
                info!("Skipping asset {:?} by filter", asset.name);
                let destination_file_name = release_directory.join(&asset.name);
                if destination_file_name.exists() {
                    info!("Cleaning up unwanted asset {:?}", &destination_file_name);
                    if let Err(err) = std::fs::remove_file(&destination_file_name) {
                        warn!("Failed to remove {:?}: {}", destination_file_name, err)
                    }
                }

                continue;
            }

            if let Err(err) = asset.download(storage, &repository.path, self) {
                warn!(
                    "Failed to download {}, skipping: {}",
                    asset.browser_download_url, err
                )
            }
        }
    }
}

impl Repository {
    pub fn mirror(&self, config: &Config, storage: &Storage) {
        info!("Processing repository {:?}", &self.path);
        let releases = match list_releases(&self.path) {
            Ok(v) => v,
            Err(err) => {
                crit!("Failed to get releases list for {}: {}", self.path, err);
                return;
            }
        };

        for release in &releases {
            release.mirror(config, storage, self)
        }
    }
}

impl Application {
    fn mirror(&self, config_path: &std::path::Path) {
        let config = Config::read(config_path).expect("Config");
        let storage = Storage::init(&config.storage).expect("Storage");

        for repo in &config.repositories {
            repo.mirror(&config, &storage)
        }
    }

    pub fn run(&self) {
        let logger = slog_syslog::SyslogBuilder::new()
            .facility(slog_syslog::Facility::LOG_USER)
            .level(slog::Level::Info)
            .unix("/dev/log")
            .start()
            .expect("Logger");

        let logger = slog::Logger::root(logger.fuse(), o!());
        let _logger_guard = slog_scope::set_global_logger(logger);

        match self {
            Application::ListReleases(v) => cmd_list_releases(v).expect("Releases list"),
            Application::Mirror(v) => self.mirror(&v.config_path),
        }
    }
}

fn main() {
    Application::parse().run()
}

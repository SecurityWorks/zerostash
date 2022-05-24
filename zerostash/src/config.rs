//! Zerostash Config
//!
//! See instructions in `commands.rs` to specify the path to your
//! application's configuration file and/or command-line options
//! for specifying it.

use anyhow::{Context, Result};
use infinitree::backends::Region;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, num::NonZeroUsize, path::PathBuf, sync::Arc};

/// Zerostash Configuration
#[derive(Default, Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ZerostashConfig {
    /// An example configuration section
    #[serde(rename = "stash", default)]
    stashes: HashMap<String, Stash>,
}

/// Describe the configuration for a named stash
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Stash {
    /// Key descriptor to use while opening the stash
    pub key: Key,
    /// Backend configuration for the stash
    pub backend: Backend,
}

impl Stash {
    /// Try to open a stash with the config-stored credentials
    pub fn try_open(&self) -> Result<crate::Stash> {
        let key = {
            use Key::*;
            match &self.key {
                Interactive => ask_credentials()?,
                Plaintext { user, password } => (user.to_string(), password.to_string()),
            }
        };

        let backend = self.backend.to_infinitree()?;

        let stash = crate::Stash::open(
            backend.clone(),
            infinitree::Key::from_credentials(&key.0, &key.1)?,
        )
        .unwrap_or_else(move |_| {
            crate::Stash::empty(
                backend,
                infinitree::Key::from_credentials(&key.0, &key.1).unwrap(),
            )
            .unwrap()
        });
        Ok(stash)
    }
}

/// Ask for credentials on the standard input using [rpassword]
pub fn ask_credentials() -> Result<(String, String)> {
    let username = rprompt::prompt_reply_stderr("Username: ")?;
    let password = rpassword::prompt_password("Password: ")?;
    Ok((username, password))
}

/// Credentials for a stash
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(tag = "source")]
pub enum Key {
    /// Plain text username/password pair
    #[serde(rename = "plaintext")]
    #[allow(missing_docs)]
    Plaintext { user: String, password: String },

    /// Get credentials through other interactive/command line methods
    #[serde(rename = "ask")]
    Interactive,
}

/// Backend configuration
/// This may be specific to the backend type
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(tag = "type")]
pub enum Backend {
    /// Use a directory on a local filesystem
    #[serde(rename = "fs")]
    #[allow(missing_docs)]
    Filesystem { path: String },

    /// Descriptor for S3 connection.
    #[serde(rename = "s3")]
    S3 {
        /// name of the bucket
        bucket: String,

        /// May be "protocol://fqdn" syntax.
        /// Supports AWS, DigitalOcean, Yandex, WasabiSys canonical names
        region: Region,

        /// ("access_key_id", "secret_access_key")
        keys: Option<(String, String)>,
    },

    /// Cache files in a local directory, up to `max_size` in size
    /// You will typically want this to be larger than the index size.
    #[serde(rename = "fs_cache")]
    FsCache {
        /// Max size of local cache
        max_size_mb: NonZeroUsize,
        /// Where to store local files
        path: String,
        /// Long-term backend
        upstream: Box<Backend>,
    },
}

impl Backend {
    fn to_infinitree(&self) -> Result<Arc<dyn infinitree::Backend>> {
        use Backend::*;

        let backend: Arc<dyn infinitree::Backend> = match self {
            Filesystem { path } => infinitree::backends::Directory::new(path)?,
            S3 {
                bucket,
                region,
                keys,
            } => {
                use infinitree::backends::{Credentials, S3};

                match keys {
                    Some((access_key, secret_key)) => S3::with_credentials(
                        region.clone(),
                        bucket,
                        Credentials::new(access_key, secret_key),
                    ),
                    None => S3::new(region.clone(), bucket),
                }
                .context("Failed to connect to S3")?
            }
            FsCache {
                max_size_mb,
                path,
                upstream,
            } => infinitree::backends::Cache::new(
                path,
                NonZeroUsize::new(max_size_mb.get() * 1024 * 1024)
                    .expect("Deserialization should have failed if `max_size_mb` is 0"),
                upstream.to_infinitree()?,
            )?,
        };

        Ok(backend)
    }
}

impl ZerostashConfig {
    /// Path to the configuration directory
    #[cfg(unix)]
    pub fn path() -> PathBuf {
        xdg::BaseDirectories::with_prefix("zerostash")
            .unwrap()
            .place_config_file("config.toml")
            .expect("cannot create configuration directory")
    }

    /// Path to the configuration directory
    #[cfg(windows)]
    pub fn path() -> PathBuf {
        let mut p = dirs::home_dir().expect("cannot find home directory");

        p.push(".zerostash");
        std::fs::create_dir_all(&p).expect("failed to create config dir");

        p.push("config.toml");
        p
    }

    /// Write the config file to the file system
    pub fn write(&self) -> Result<()> {
        unimplemented!()
    }

    /// Find a stash by name in the config, and return a read-only
    /// reference if found
    pub fn resolve_stash(&self, alias: impl AsRef<str>) -> Option<Stash> {
        self.stashes.get(alias.as_ref()).cloned()
    }
}

mod tests {
    #[test]
    fn can_parse_config() {
        use super::ZerostashConfig;
        use abscissa_core::Config;

        ZerostashConfig::load_toml(
            r#"
[stash.first]
key = { source = "plaintext", user = "123", password = "123"}
backend = { type = "fs", path = "/path/to/stash" }

[stash.second]
key = { source = "ask"}
backend = { type = "fs", path = "/path/to/stash" }

[stash.s3]
key = { source = "ask" }
backend = { type = "s3", bucket = "test_bucket", region = { name = "us-east-1" }, keys = ["access_key_id", "secret_key"] }

[stash.s3_env_key]
key = { source = "ask" }
backend = { type = "s3", bucket = "test_bucket", region = { name = "us-east-1" } }

[stash.s3_cached]
key = { source = "ask" }

[stash.s3_cached.backend]
type = "fs_cache"
path = "/path_to_stash"
max_size_mb = 1024

[stash.s3_cached.backend.upstream]
type = "s3"
bucket = "test_bucket"
region = { name = "custom", details = { endpoint = "https://127.0.0.1:8080/", "region" = "" }}
"#,
        )
        .unwrap();
    }

    #[test]
    fn can_load_empty() {
        use super::ZerostashConfig;
        use abscissa_core::Config;

        ZerostashConfig::load_toml(r#""#).unwrap();
    }
}

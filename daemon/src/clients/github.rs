use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{
    Deserialize,
    Serialize,
};
use tokio_util::bytes::Bytes;

use crate::api::ApiErrorExt;

const GITHUB_BASE_URL: &str = "https://api.github.com";
const GITHUB_CLIENT_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ReleaseAsset {
    pub url: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct GithubRelease {
    pub html_url: String,
    pub tag_name: String,
    pub assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct GithubError {
    pub message: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
enum GithubResponse<T> {
    Ok(T),
    Err(GithubError),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GithubClientError {
    #[error("Unknown API error")]
    UnknownApiError,
    #[error("Request failed")]
    RequestFailed,
    #[error("API error: {message}")]
    ApiError { message: String, status: String },
    #[error("Release not found for any of versions {versions:?} in {repo_url} repository")]
    ReleaseNotFound { versions: Vec<u8>, repo_url: String },
    #[error("Asset {asset_name} not found in release {release_url}")]
    AssetNotFound {
        release_url: String,
        asset_name: String,
    },
}

impl From<reqwest::Error> for GithubClientError {
    fn from(_value: reqwest::Error) -> Self {
        Self::RequestFailed
    }
}

impl From<GithubError> for GithubClientError {
    fn from(value: GithubError) -> Self {
        Self::ApiError {
            message: value.message,
            status: value.status,
        }
    }
}

impl ApiErrorExt for GithubClientError {
    fn category(&self) -> &str {
        match self {
            Self::UnknownApiError
            | Self::ApiError {
                ..
            }
            | Self::RequestFailed => "INTERNAL_SERVER_ERROR",
            Self::ReleaseNotFound {
                ..
            }
            | Self::AssetNotFound {
                ..
            } => "ENTITY_NOT_FOUND",
        }
    }

    fn code(&self) -> &str {
        match self {
            Self::UnknownApiError
            | Self::ApiError {
                ..
            }
            | Self::RequestFailed => "INTERNAL_SERVER_ERROR",
            Self::ReleaseNotFound {
                ..
            } => "PLUGIN_RELEASE_NOT_FOUND",
            Self::AssetNotFound {
                ..
            } => "PLUGIN_ASSET_NOT_FOUND_IN_RELEASE",
        }
    }

    #[expect(unused_variables)]
    fn message(&self) -> &str {
        match self {
            Self::UnknownApiError
            | Self::ApiError {
                ..
            }
            | Self::RequestFailed => "Failed to get plugin of supported version",
            Self::ReleaseNotFound {
                versions, ..
            } => {
                // TODO: return Cow instead of static str from such methods
                // let versions = versions
                //     .iter()
                //     .map(|version| format!("v{version}"))
                //     .collect::<Vec<_>>()
                //     .join(", ");

                // &format!("Plugin wans't found for any of supported versions: {versions}")
                "Plugin wasn't found for any of supported versions"
            },
            Self::AssetNotFound {
                release_url,
                asset_name,
            } => {
                // &format!("Required asset {asset_name} wans't found in plugin release:
                // {release_url}")
                "Required asset wasn't found in plugin release"
            },
        }
    }

    fn http_status_code(&self) -> reqwest::StatusCode {
        match self {
            Self::UnknownApiError
            | Self::ApiError {
                ..
            }
            | Self::RequestFailed => reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            Self::ReleaseNotFound {
                ..
            }
            | Self::AssetNotFound {
                ..
            } => reqwest::StatusCode::NOT_FOUND,
        }
    }
}

#[derive(Clone)]
pub struct GithubClient {
    client: reqwest::Client,
    base_url: String,
}

impl GithubClient {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: GITHUB_BASE_URL.to_string(),
        }
    }

    #[tracing::instrument(skip(self))]
    async fn get_asset_zip(
        &self,
        url: String,
    ) -> Result<Bytes, GithubClientError> {
        let result = self
            .client
            .get(&url)
            .header("Accept", "application/octet-stream")
            .header("User-Agent", "Kalatori")
            .timeout(GITHUB_CLIENT_REQUEST_TIMEOUT)
            .send()
            .await
            .inspect_err(|e| {
                tracing::warn!(
                    error.source = ?e,
                    "Github asset request failed"
                )
            })?
            .bytes()
            .await
            .inspect_err(|e| {
                tracing::warn!(
                    error.source = ?e,
                    "Failed to fetch bytes from github asset request"
                )
            })?;

        Ok(result)
    }

    #[tracing::instrument(skip(self))]
    async fn send_request<R>(
        &self,
        url: &str,
    ) -> Result<R, GithubClientError>
    where
        R: DeserializeOwned + std::fmt::Debug,
    {
        let full_url = format!("{}{}", self.base_url, url);

        let raw_response = self
            .client
            .get(full_url)
            .header("User-Agent", "Kalatori")
            .timeout(GITHUB_CLIENT_REQUEST_TIMEOUT)
            .send()
            .await
            .inspect_err(|e| {
                tracing::warn!(
                    error.source = ?e,
                    "Github request failed"
                )
            })?
            .text()
            .await?;

        tracing::trace!(
            text = %raw_response,
            "Got raw response text from github"
        );

        let response = serde_json::from_str(&raw_response).map_err(|e| {
            tracing::error!(
                text = %raw_response,
                error.source = ?e,
                "Error while trying to deserialize response from github"
            );

            GithubClientError::UnknownApiError
        })?;

        tracing::trace!(
            ?response,
            "Got parsed response from github"
        );

        match response {
            GithubResponse::Ok(data) => Ok(data),
            GithubResponse::Err(e) => Err(e.into()),
        }
    }

    #[tracing::instrument(skip(self))]
    async fn get_releases(
        &self,
        repo: &str,
    ) -> Result<Vec<GithubRelease>, GithubClientError> {
        let url = format!("/repos/{repo}/releases");
        self.send_request(&url).await
    }

    #[tracing::instrument(skip(self))]
    fn find_release_asset(
        &self,
        releases: Vec<GithubRelease>,
        versions: &[u8],
        asset_name: &str,
        repo: &str,
    ) -> Result<String, GithubClientError> {
        let release = releases
            .into_iter()
            .find(|release| {
                versions.iter().any(|version| {
                    release
                        .tag_name
                        .starts_with(&format!("v{}", version))
                })
            })
            .ok_or_else(|| GithubClientError::ReleaseNotFound {
                versions: versions.to_vec(),
                repo_url: format!("https://github.com/{repo}/releases"),
            })?;

        release
            .assets
            .into_iter()
            .find(|asset| asset.name == asset_name)
            .map(|asset| asset.url)
            .ok_or_else(|| GithubClientError::AssetNotFound {
                release_url: release.html_url,
                asset_name: asset_name.to_string(),
            })
    }

    #[tracing::instrument(skip(self))]
    pub async fn find_and_fetch_plugin(
        &self,
        repo: &str,
        versions: &[u8],
        asset_name: &str,
    ) -> Result<Bytes, GithubClientError> {
        let releases = self.get_releases(repo).await?;
        let asset_url = self.find_release_asset(releases, versions, asset_name, repo)?;
        self.get_asset_zip(asset_url).await
    }
}

#[cfg(test)]
mod tests {
    use httpmock::Method::GET;
    use httpmock::MockServer;

    use super::*;

    fn default_github_releases(base_url: &str) -> Vec<GithubRelease> {
        vec![
            GithubRelease {
                html_url: format!(
                    "{base_url}/Kalapaja/kalatori-woocommerce-plugin/releases/v2.0.0"
                ),
                tag_name: "v2.0.0".to_string(),
                assets: vec![
                    ReleaseAsset {
                        url: format!(
                            "{base_url}/repos/Kalapaja/kalatori-woocommerce-plugin/releases/assets/3"
                        ),
                        name: "Source code (zip)".to_string(),
                    },
                    ReleaseAsset {
                        url: format!(
                            "{base_url}/repos/Kalapaja/kalatori-woocommerce-plugin/releases/assets/4"
                        ),
                        name: "kalatori-woocommerce-plugin.zip".to_string(),
                    },
                ],
            },
            GithubRelease {
                html_url: format!(
                    "{base_url}/Kalapaja/kalatori-woocommerce-plugin/releases/v1.0.7"
                ),
                tag_name: "v1.0.7".to_string(),
                assets: vec![
                    ReleaseAsset {
                        url: format!(
                            "{base_url}/repos/Kalapaja/kalatori-woocommerce-plugin/releases/assets/1"
                        ),
                        name: "Source code (zip)".to_string(),
                    },
                    ReleaseAsset {
                        url: format!(
                            "{base_url}/repos/Kalapaja/kalatori-woocommerce-plugin/releases/assets/2"
                        ),
                        name: "kalatori-woocommerce-plugin.zip".to_string(),
                    },
                ],
            },
        ]
    }

    #[tracing_test::traced_test]
    #[tokio::test]
    async fn test_get_asset_zip() {
        let mut client = GithubClient::new();
        let mock_server = MockServer::start();
        let base_url = mock_server.base_url();
        client.base_url = base_url.clone();

        let mock = mock_server.mock(|when, then| {
            when.method(GET)
                .header_exists("User-Agent")
                .header("Accept", "application/octet-stream")
                .path("/test");

            then.body([8, 10, 12, 3]);
        });

        let result = client
            .get_asset_zip(format!("{base_url}/test"))
            .await
            .unwrap();
        assert_eq!(result.to_vec(), &[8, 10, 12, 3]);
        mock.assert();
    }

    #[tokio::test]
    async fn test_get_releases() {
        let mut client = GithubClient::new();
        let mock_server = MockServer::start();
        let base_url = mock_server.base_url();
        client.base_url = base_url.clone();

        let mut mock = mock_server.mock(|when, then| {
            when.method(GET)
                .header_exists("User-Agent")
                .path("/repos/Kalapaja/kalatori-woocommerce-plugin/releases");

            then.json_body_obj(&default_github_releases(&base_url));
        });

        let releases = client
            .get_releases("Kalapaja/kalatori-woocommerce-plugin")
            .await
            .unwrap();

        assert_eq!(
            releases,
            default_github_releases(&base_url)
        );
        mock.assert();
        mock.delete();

        let mut mock = mock_server.mock(|when, then| {
            when.method(GET)
                .header_exists("User-Agent")
                .path("/repos/Kalapaja/kalatori-woocommerce-plugin/releases");

            then.json_body_obj(&GithubError {
                message: "Not found".to_string(),
                status: "404".to_string(),
            });
        });

        let result = client
            .get_releases("Kalapaja/kalatori-woocommerce-plugin")
            .await
            .unwrap_err();

        assert_eq!(
            result,
            GithubClientError::ApiError {
                message: "Not found".to_string(),
                status: "404".to_string(),
            }
        );

        mock.assert();
        mock.delete();

        let mock = mock_server.mock(|when, then| {
            when.method(GET)
                .header_exists("User-Agent")
                .path("/repos/Kalapaja/kalatori-woocommerce-plugin/releases");

            then.body("error");
        });

        let result = client
            .get_releases("Kalapaja/kalatori-woocommerce-plugin")
            .await
            .unwrap_err();

        assert_eq!(
            result,
            GithubClientError::UnknownApiError
        );

        mock.assert();
    }

    #[test]
    fn test_find_release_asset() {
        let client = GithubClient::new();

        let releases = default_github_releases("http://test.com");

        let asset_url = client
            .find_release_asset(
                releases.clone(),
                &[1, 2],
                "kalatori-woocommerce-plugin.zip",
                "test/repo",
            )
            .unwrap();

        assert_eq!(
            asset_url,
            "http://test.com/repos/Kalapaja/kalatori-woocommerce-plugin/releases/assets/4"
        );

        let asset_url = client
            .find_release_asset(
                releases.clone(),
                &[1],
                "kalatori-woocommerce-plugin.zip",
                "test/repo",
            )
            .unwrap();

        assert_eq!(
            asset_url,
            "http://test.com/repos/Kalapaja/kalatori-woocommerce-plugin/releases/assets/2"
        );

        let error = client
            .find_release_asset(
                releases.clone(),
                &[3],
                "kalatori-woocommerce-plugin.zip",
                "test/repo",
            )
            .unwrap_err();

        assert_eq!(
            error,
            GithubClientError::ReleaseNotFound {
                versions: vec![3],
                repo_url: "https://github.com/test/repo/releases".to_string(),
            }
        );

        let error = client
            .find_release_asset(
                releases.clone(),
                &[2],
                "test.zip",
                "test/repo",
            )
            .unwrap_err();

        assert_eq!(
            error,
            GithubClientError::AssetNotFound {
                release_url: "http://test.com/Kalapaja/kalatori-woocommerce-plugin/releases/v2.0.0"
                    .to_string(),
                asset_name: "test.zip".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn test_find_and_fetch_plugin() {
        let mut client = GithubClient::new();
        let mock_server = MockServer::start();
        let base_url = mock_server.base_url();
        client.base_url = base_url.clone();

        let releases_mock = mock_server.mock(|when, then| {
            when.method(GET)
                .header_exists("User-Agent")
                .path("/repos/Kalapaja/kalatori-woocommerce-plugin/releases");

            then.json_body_obj(&default_github_releases(&base_url));
        });

        let asset_mock = mock_server.mock(|when, then| {
            when.method(GET)
                .header_exists("User-Agent")
                .header("Accept", "application/octet-stream")
                .path("/repos/Kalapaja/kalatori-woocommerce-plugin/releases/assets/4");

            then.body([8, 10, 12, 3]);
        });

        let result = client
            .find_and_fetch_plugin(
                "Kalapaja/kalatori-woocommerce-plugin",
                &[1, 2],
                "kalatori-woocommerce-plugin.zip",
            )
            .await
            .unwrap();

        assert_eq!(result.to_vec(), &[8, 10, 12, 3]);
        releases_mock.assert();
        asset_mock.assert();
    }
}

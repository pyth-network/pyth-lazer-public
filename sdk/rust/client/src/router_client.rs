use {
    anyhow::bail,
    pyth_lazer_protocol::api::SignedGuardianSetUpgrade,
    reqwest::StatusCode,
    serde::{Deserialize, Serialize},
    std::{sync::Arc, time::Duration},
    tracing::warn,
    url::Url,
};

/// Configuration for the router client.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PythLazerRouterClientConfig {
    /// URLs of the router services.
    pub urls: Vec<Url>,
    /// Timeout of an individual request.
    #[serde(with = "humantime_serde", default = "default_request_timeout")]
    pub request_timeout: Duration,
    /// Access token for authenticated endpoints.
    pub access_token: String,
}

fn default_request_timeout() -> Duration {
    Duration::from_secs(15)
}

/// Client for router HTTP API endpoints.
#[derive(Debug, Clone)]
pub struct PythLazerRouterClient {
    config: Arc<PythLazerRouterClientConfig>,
    client: reqwest::Client,
}

impl PythLazerRouterClient {
    pub fn new(config: PythLazerRouterClientConfig) -> anyhow::Result<Self> {
        if config.urls.is_empty() {
            bail!("no router urls provided");
        }
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(config.request_timeout)
                .build()?,
            config: Arc::new(config),
        })
    }

    /// Fetch the current guardian set upgrade from a router, if one is in progress.
    ///
    /// Returns `Ok(None)` if no upgrade is currently in progress (404 response).
    ///
    /// Tries each configured URL in order, falling back to subsequent ones on failure.
    pub async fn guardian_set_upgrade(&self) -> anyhow::Result<Option<SignedGuardianSetUpgrade>> {
        for url in &self.config.urls {
            match self.request_guardian_set_upgrade(url).await {
                Ok(output) => return Ok(output),
                Err(err) => {
                    warn!(?url, ?err, "failed to fetch from router, trying next url");
                }
            }
        }
        bail!(
            "failed to fetch data from any router urls ({:?})",
            self.config.urls
        );
    }

    async fn request_guardian_set_upgrade(
        &self,
        url: &Url,
    ) -> anyhow::Result<Option<SignedGuardianSetUpgrade>> {
        let url = url.join("v1/guardian_set_upgrade")?;

        let response = self
            .client
            .get(url.clone())
            .bearer_auth(&self.config.access_token)
            .send()
            .await?;

        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }

        let response = response.error_for_status()?;
        let upgrade = response.json::<SignedGuardianSetUpgrade>().await?;
        Ok(Some(upgrade))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_guardian_set_upgrade_success() {
        let server = httpmock::MockServer::start();

        let json_body = serde_json::json!({
            "current_guardian_set_index": 1,
            "new_guardian_set_index": 2,
            "new_guardian_keys": ["aabbccdd"],
            "body": "deadbeef",
            "signature": "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f4041"
        });

        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/v1/guardian_set_upgrade")
                .header("Authorization", "Bearer test-token");
            then.status(200)
                .header("Content-Type", "application/json")
                .json_body(json_body);
        });

        let client = PythLazerRouterClient::new(PythLazerRouterClientConfig {
            urls: vec![Url::parse(&server.base_url()).unwrap()],
            request_timeout: Duration::from_secs(5),
            access_token: "test-token".to_string(),
        })
        .unwrap();

        let result = client.guardian_set_upgrade().await.unwrap();
        assert!(result.is_some());
        let upgrade = result.unwrap();
        assert_eq!(upgrade.current_guardian_set_index, 1);
        assert_eq!(upgrade.new_guardian_set_index, 2);
        assert_eq!(upgrade.new_guardian_keys.len(), 1);
    }

    #[tokio::test]
    async fn test_guardian_set_upgrade_not_found() {
        let server = httpmock::MockServer::start();

        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/v1/guardian_set_upgrade");
            then.status(404);
        });

        let client = PythLazerRouterClient::new(PythLazerRouterClientConfig {
            urls: vec![Url::parse(&server.base_url()).unwrap()],
            request_timeout: Duration::from_secs(5),
            access_token: "test-token".to_string(),
        })
        .unwrap();

        let result = client.guardian_set_upgrade().await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_guardian_set_upgrade_fallback_to_second_url() {
        let server1 = httpmock::MockServer::start();
        let server2 = httpmock::MockServer::start();

        // First server returns 400 (error, triggers fallback)
        server1.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/v1/guardian_set_upgrade");
            then.status(400);
        });

        let json_body = serde_json::json!({
            "current_guardian_set_index": 10,
            "new_guardian_set_index": 11,
            "new_guardian_keys": ["ff00ff00"],
            "body": "cafe",
            "signature": "0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f4041"
        });

        // Second server returns success
        server2.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/v1/guardian_set_upgrade");
            then.status(200)
                .header("Content-Type", "application/json")
                .json_body(json_body);
        });

        let client = PythLazerRouterClient::new(PythLazerRouterClientConfig {
            urls: vec![
                Url::parse(&server1.base_url()).unwrap(),
                Url::parse(&server2.base_url()).unwrap(),
            ],
            request_timeout: Duration::from_secs(5),
            access_token: "test-token".to_string(),
        })
        .unwrap();

        let result = client.guardian_set_upgrade().await.unwrap();
        assert!(result.is_some());
        let upgrade = result.unwrap();
        assert_eq!(upgrade.current_guardian_set_index, 10);
        assert_eq!(upgrade.new_guardian_set_index, 11);
    }

    #[test]
    fn test_new_no_urls_returns_error() {
        let result = PythLazerRouterClient::new(PythLazerRouterClientConfig {
            urls: vec![],
            request_timeout: Duration::from_secs(5),
            access_token: "test-token".to_string(),
        });

        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("no router urls provided")
        );
    }
}

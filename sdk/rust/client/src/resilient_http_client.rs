use {
    anyhow::{Context as _, bail},
    atomicwrites::replace_atomic,
    backoff::{SystemClock, exponential::ExponentialBackoff, future::retry_notify},
    futures::{Stream, StreamExt, future::BoxFuture, stream::FuturesUnordered},
    serde::{Serialize, de::DeserializeOwned},
    std::{
        future::Future,
        io::Write,
        path::{Path, PathBuf},
        sync::Arc,
        time::Duration,
    },
    tokio::{sync::mpsc, time::sleep},
    tokio_stream::wrappers::ReceiverStream,
    tracing::{Instrument, info, info_span, warn},
    url::Url,
};

/// Shared HTTP fetch / cache / stream infrastructure used by the api and state clients.
/// Provides multi-URL failover, exponential backoff, on-disk caching, and a polling stream.
#[derive(Debug, Clone)]
pub(crate) struct ResilientHttpClient {
    pub urls: Vec<Url>,
    pub update_interval: Duration,
    pub channel_capacity: usize,
    pub cache_dir: Option<PathBuf>,
    pub reqwest: reqwest::Client,
}

impl ResilientHttpClient {
    pub fn cache_file_path(&self, name: &str) -> Option<PathBuf> {
        self.cache_dir.as_ref().map(|d| d.join(name))
    }

    /// Uses all configured URLs to perform request `f` and handles retrying on error.
    /// Returns the value once any of the requests succeeds. If all requests fail,
    /// tries to fetch the data from local cache. If loading from cache also fails,
    /// it keeps retrying the requests until any of them succeeds.
    pub async fn fetch_or_cache<'a, F, Fut, R>(
        &'a self,
        cache_file_path: Option<PathBuf>,
        f: F,
    ) -> anyhow::Result<R>
    where
        F: Fn(&'a ResilientHttpClient, &'a Url) -> Fut,
        Fut: Future<Output = Result<R, backoff::Error<anyhow::Error>>>,
        R: Serialize + DeserializeOwned,
    {
        match self.fetch_from_all_urls(true, &f).await {
            Ok(data) => {
                info!("fetched initial data");
                if let Some(cache_file_path) = cache_file_path
                    && let Err(err) = atomic_save_file::<R>(&cache_file_path, &data)
                {
                    warn!(?err, ?cache_file_path, "failed to save data to cache file");
                }
                return Ok(data);
            }
            Err(err) => warn!(?err, "all requests failed"),
        }

        if let Some(cache_file_path) = cache_file_path {
            match load_file::<R>(&cache_file_path) {
                Ok(Some(data)) => {
                    info!("failed to fetch initial data, but loaded last known data from cache");
                    return Ok(data);
                }
                Ok(None) => info!("no data found in cache"),
                Err(err) => warn!(?err, "failed to fetch data from cache"),
            }
        }

        self.fetch_from_all_urls(false, f).await
    }

    /// Creates a fault-tolerant stream that requests data using `f` and yields new items
    /// when a change of value occurs.
    ///
    /// Returns an error if the initial fetch failed.
    /// On a successful return, the channel will always contain the initial data that can be fetched
    /// immediately from the returned stream.
    /// You should continuously poll the stream to receive updates.
    pub async fn stream<F, R>(
        self: &Arc<Self>,
        cache_file_path: Option<PathBuf>,
        f: F,
    ) -> anyhow::Result<impl Stream<Item = R> + use<F, R> + Unpin>
    where
        for<'a> F: Fn(
                &'a ResilientHttpClient,
                &'a Url,
            ) -> BoxFuture<'a, Result<R, backoff::Error<anyhow::Error>>>
            + Send
            + Sync
            + 'static,
        R: Clone + Serialize + DeserializeOwned + PartialEq + Send + Sync + 'static,
    {
        if self.channel_capacity == 0 {
            bail!("channel_capacity cannot be 0");
        }
        let value = self.fetch_or_cache(cache_file_path.clone(), &f).await?;
        let (sender, receiver) = mpsc::channel(self.channel_capacity);

        let previous_value = value.clone();
        sender
            .send(value)
            .await
            .expect("send to new channel failed");
        let http = Arc::clone(self);
        #[allow(clippy::disallowed_methods, reason = "instrumented")]
        tokio::spawn(
            async move {
                http.keep_stream_updated(cache_file_path, sender, previous_value, &f)
                    .await;
            }
            .instrument(info_span!("fetch task", urls = ?self.urls)),
        );
        Ok(ReceiverStream::new(receiver))
    }

    async fn keep_stream_updated<F, R>(
        &self,
        cache_file_path: Option<PathBuf>,
        sender: mpsc::Sender<R>,
        mut previous_data: R,
        f: &F,
    ) where
        for<'a> F: Fn(
            &'a ResilientHttpClient,
            &'a Url,
        ) -> BoxFuture<'a, Result<R, backoff::Error<anyhow::Error>>>,
        R: Serialize + DeserializeOwned + PartialEq + Clone,
    {
        info!("starting background task for updating data");
        loop {
            sleep(self.update_interval).await;
            if sender.is_closed() {
                info!("data handle dropped, stopping background task");
                return;
            }
            match self.fetch_from_all_urls(true, f).await {
                Ok(new_data) => {
                    if previous_data != new_data {
                        info!("data changed");
                        if let Some(cache_file_path) = &cache_file_path
                            && let Err(err) = atomic_save_file(cache_file_path, &new_data)
                        {
                            warn!(?err, ?cache_file_path, "failed to save data to cache file");
                        }

                        previous_data = new_data.clone();
                        if sender.send(new_data.clone()).await.is_err() {
                            info!("update handle dropped, stopping background task");
                            return;
                        }
                    }
                }
                Err(err) => warn!(?err, "failed to fetch data"),
            }
        }
    }

    /// Uses all configured URLs to perform request `f` and handles retrying on error.
    ///
    /// Returns the value once any of the requests succeeds.
    /// If `limit_by_update_interval` is true, the total time spent retrying is limited to
    /// `self.update_interval`. If false, the requests will be retried indefinitely.
    async fn fetch_from_all_urls<'a, F, Fut, R>(
        &'a self,
        limit_by_update_interval: bool,
        f: F,
    ) -> anyhow::Result<R>
    where
        F: Fn(&'a ResilientHttpClient, &'a Url) -> Fut,
        Fut: Future<Output = Result<R, backoff::Error<anyhow::Error>>>,
    {
        if self.urls.is_empty() {
            bail!("no urls provided");
        }
        let mut futures = self
            .urls
            .iter()
            .map(|url| {
                Box::pin(
                    self.fetch_from_single_url_with_retry(limit_by_update_interval, || {
                        f(self, url)
                    }),
                )
            })
            .collect::<FuturesUnordered<_>>();
        while let Some(result) = futures.next().await {
            match result {
                Ok(output) => return Ok(output),
                Err(err) => warn!("request failed: {:?}", err),
            }
        }

        bail!("failed to fetch data from any urls ({:?})", self.urls);
    }

    async fn fetch_from_single_url_with_retry<F, Fut, R>(
        &self,
        limit_by_update_interval: bool,
        f: F,
    ) -> anyhow::Result<R>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<R, backoff::Error<anyhow::Error>>>,
    {
        let mut backoff = ExponentialBackoff::<SystemClock>::default();
        if limit_by_update_interval {
            backoff.max_elapsed_time = Some(self.update_interval);
        }
        retry_notify(backoff, f, |e, _| warn!(?e, "operation failed, will retry")).await
    }
}

pub(crate) trait BackoffErrorForStatusExt: Sized {
    fn backoff_error_for_status(self) -> Result<Self, backoff::Error<anyhow::Error>>;
}

impl BackoffErrorForStatusExt for reqwest::Response {
    fn backoff_error_for_status(self) -> Result<Self, backoff::Error<anyhow::Error>> {
        let status = self.status();
        self.error_for_status().map_err(|err| {
            if status.is_server_error() {
                backoff::Error::transient(err.into())
            } else {
                backoff::Error::permanent(err.into())
            }
        })
    }
}

fn load_file<T: DeserializeOwned>(path: &Path) -> anyhow::Result<Option<T>> {
    let parent_path = path.parent().context("invalid file path: no parent")?;
    fs_err::create_dir_all(parent_path)?;

    if !path.try_exists()? {
        return Ok(None);
    }
    let json_data = fs_err::read_to_string(path)?;
    let data = serde_json::from_str::<T>(&json_data)?;
    Ok(Some(data))
}

fn atomic_save_file<T: Serialize>(path: &Path, data: &T) -> anyhow::Result<()> {
    let parent_path = path.parent().context("invalid file path: no parent")?;
    fs_err::create_dir_all(parent_path)?;

    let json_data = serde_json::to_string(&data)?;
    let tmp_path = path.with_extension("tmp");
    let mut tmp_file = fs_err::File::create(&tmp_path)?;
    tmp_file.write_all(json_data.as_bytes())?;
    tmp_file.flush()?;
    tmp_file.sync_all()?;
    replace_atomic(&tmp_path, path)?;

    Ok(())
}

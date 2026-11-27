use super::image_types::{ImageError, ImagePin};
use reqwest::{Client, Url};
use std::time::Duration;

pub struct ImageHttpClient {
    client: Client,
    pin: ImagePin,
    origins: Vec<String>,
}
fn secure_url(value: &str) -> Result<Url, ImageError> {
    let url = Url::parse(value).map_err(|_| ImageError::Invalid("image URL"))?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(ImageError::Invalid("image URL policy"));
    }
    let host = url
        .host_str()
        .ok_or(ImageError::Invalid("image host"))?
        .trim_end_matches('.');
    let loopback = host.trim_matches(['[', ']']).parse::<std::net::IpAddr>().is_ok_and(|ip| {
        ip.is_loopback() || matches!(ip, std::net::IpAddr::V6(v) if v.to_ipv4_mapped().is_some_and(|v| v.is_loopback()))
    });
    if loopback || host == "localhost" || host.ends_with(".localhost") {
        return Err(ImageError::Invalid("loopback image origin"));
    }
    Ok(url)
}
fn client_builder() -> reqwest::ClientBuilder {
    Client::builder()
        .tls_backend_native()
        .https_only(true)
        .no_proxy()
        .no_gzip()
        .no_brotli()
        .no_deflate()
        .no_zstd()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(30))
}
impl ImageHttpClient {
    pub async fn manifest(
        &self,
        closed: tokio::sync::watch::Receiver<bool>,
    ) -> Result<Vec<u8>, ImageError> {
        let mut bytes = Vec::new();
        self.transfer(
            &self.pin.manifest_url,
            &mut bytes,
            8 * 1024 * 1024,
            (None, &self.pin.manifest_sha256),
            closed,
            |_| {},
        )
        .await?;
        Ok(bytes)
    }
    async fn transfer<W: tokio::io::AsyncWrite + Unpin + Send>(
        &self,
        url: &str,
        output: &mut W,
        maximum: u64,
        expected: (Option<u64>, &str),
        mut closed: tokio::sync::watch::Receiver<bool>,
        progress: impl Fn(u64) + Send + Sync,
    ) -> Result<(), ImageError> {
        use sha2::{Digest, Sha256};
        use tokio::io::AsyncWriteExt;
        let cancelled = async |closed: &mut tokio::sync::watch::Receiver<bool>| loop {
            if *closed.borrow() || closed.changed().await.is_err() {
                break;
            }
        };
        let io_error = |e: std::io::Error| {
            if e.kind() == std::io::ErrorKind::StorageFull {
                ImageError::NoSpace
            } else {
                ImageError::Storage(e.kind())
            }
        };
        let mut url = secure_url(url)?;
        for hop in 0..=5 {
            if !self.origins.contains(&url.origin().ascii_serialization()) {
                return Err(ImageError::Invalid("image redirect origin"));
            }
            let response = tokio::select! {
                biased;
                _ = cancelled(&mut closed) => return Err(ImageError::Cancelled),
                result = self.client.get(url.clone()).header(reqwest::header::ACCEPT_ENCODING, "identity").send() =>
                    result.map_err(|_| ImageError::Network("image request"))?,
            };
            if response.status().is_redirection() {
                if hop == 5 {
                    return Err(ImageError::Network("image redirect limit"));
                }
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .ok_or(ImageError::Network("image redirect location"))?;
                url = secure_url(
                    url.join(location)
                        .map_err(|_| ImageError::Invalid("image redirect URL"))?
                        .as_str(),
                )?;
                continue;
            }
            if response.status() != reqwest::StatusCode::OK {
                return Err(ImageError::Network("image status"));
            }
            if response
                .headers()
                .get_all(reqwest::header::CONTENT_ENCODING)
                .iter()
                .any(|v| !v.as_bytes().eq_ignore_ascii_case(b"identity"))
            {
                return Err(ImageError::Integrity);
            }
            if let Some(length) = response.content_length() {
                if length > maximum {
                    return Err(ImageError::LimitExceeded);
                }
                if expected.0.is_some_and(|wanted| length != wanted) {
                    return Err(ImageError::Integrity);
                }
            }
            let mut response = response;
            let mut bytes = 0u64;
            let mut hash = Sha256::new();
            loop {
                let chunk = tokio::select! {
                    biased;
                    _ = cancelled(&mut closed) => return Err(ImageError::Cancelled),
                    result = response.chunk() => result.map_err(|_| ImageError::Network("image body"))?,
                };
                let Some(chunk) = chunk else { break };
                bytes = bytes
                    .checked_add(chunk.len() as u64)
                    .filter(|n| *n <= maximum)
                    .ok_or(ImageError::LimitExceeded)?;
                tokio::select! {
                    biased;
                    _ = cancelled(&mut closed) => return Err(ImageError::Cancelled),
                    result = output.write_all(&chunk) => result.map_err(io_error)?,
                }
                hash.update(&chunk);
                progress(bytes);
            }
            if *closed.borrow() || closed.has_changed().is_err() {
                return Err(ImageError::Cancelled);
            }
            if expected.0.is_some_and(|n| n != bytes)
                || format!("{:x}", hash.finalize()) != expected.1
            {
                return Err(ImageError::Integrity);
            }
            output.flush().await.map_err(io_error)?;
            return Ok(());
        }
        unreachable!("bounded redirect loop returns on its final hop")
    }
    pub fn new(pin: &ImagePin) -> Result<Self, ImageError> {
        let manifest = secure_url(&pin.manifest_url)?;
        let base = secure_url(&pin.asset_base)?;
        if !base.path().ends_with('/') || base.query().is_some() {
            return Err(ImageError::Invalid("asset base"));
        }
        let mut origins = vec![
            manifest.origin().ascii_serialization(),
            base.origin().ascii_serialization(),
        ];
        for origin in &pin.redirect_origins {
            let url = secure_url(origin)?;
            if url.path() != "/" || url.query().is_some() {
                return Err(ImageError::Invalid("redirect origin"));
            }
            origins.push(url.origin().ascii_serialization());
        }
        let client = client_builder()
            .build()
            .map_err(|_| ImageError::Unsupported)?;
        Ok(Self {
            client,
            pin: pin.clone(),
            origins,
        })
    }
}

type Decoder = std::sync::mpsc::SyncSender<Vec<u8>>;
struct AssetWriter {
    file: std::sync::Arc<std::fs::File>,
    owner: std::sync::Arc<super::image_cache::CacheTransaction>,
    decoder: Option<Decoder>,
    pending: Option<tokio::task::JoinHandle<std::io::Result<usize>>>,
}
fn asset_writer(
    file: std::fs::File,
    owner: std::sync::Arc<super::image_cache::CacheTransaction>,
    decoder: Option<Decoder>,
) -> AssetWriter {
    AssetWriter {
        file: std::sync::Arc::new(file),
        owner,
        decoder,
        pending: None,
    }
}
/// Chunk bytes as they are written; the last sender dropping ends the stream.
struct Received {
    receive: std::sync::mpsc::Receiver<Vec<u8>>,
    bytes: Vec<u8>,
    offset: usize,
}
impl std::io::Read for Received {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        while self.offset == self.bytes.len() {
            let Ok(bytes) = self.receive.recv() else {
                return Ok(0);
            };
            (self.bytes, self.offset) = (bytes, 0);
        }
        let n = buffer.len().min(self.bytes.len() - self.offset);
        buffer[..n].copy_from_slice(&self.bytes[self.offset..self.offset + n]);
        self.offset += n;
        Ok(n)
    }
}
impl tokio::io::AsyncWrite for AssetWriter {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bytes: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        use std::{future::Future, io::Write, task::Poll};
        if self.pending.is_none() {
            if bytes.is_empty() {
                return Poll::Ready(Ok(0));
            }
            let (file, owner, mut bytes) = (self.file.clone(), self.owner.clone(), bytes.to_vec());
            let decoder = self.decoder.clone();
            // The actual I/O job owns the pin even if the caller drops its future.
            self.pending = Some(tokio::task::spawn_blocking(move || {
                let _owner = owner;
                let n = (&*file).write(&bytes)?;
                bytes.truncate(n);
                // A full queue holds the download back until the decoder catches up.
                match decoder {
                    Some(decoder) if n > 0 => decoder
                        .send(bytes)
                        .map(|_| n)
                        .map_err(|_| std::io::ErrorKind::BrokenPipe.into()),
                    _ => Ok(n),
                }
            }));
        }
        match std::pin::Pin::new(self.pending.as_mut().unwrap()).poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(result) => {
                self.pending = None;
                Poll::Ready(
                    result.unwrap_or_else(|_| Err(std::io::Error::other("image writer stopped"))),
                )
            }
        }
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        // std::fs::File is unbuffered; wait for any outstanding blocking write.
        self.poll_write(cx, &[]).map(|result| result.map(|_| ()))
    }
    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        self.poll_flush(cx)
    }
}

pub(crate) async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, ImageError> + Send + 'static,
) -> Result<T, ImageError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| ImageError::Storage(std::io::ErrorKind::Other))?
}
pub async fn acquire_image(
    image: &super::image_types::ResolvedImage,
    cache: &super::image_cache::ImageCache,
    client: &ImageHttpClient,
    closed: tokio::sync::watch::Receiver<bool>,
    progress: impl Fn(super::image_types::ImageProgress) + Send + Sync,
) -> Result<super::image_types::VerifiedImageSet, ImageError> {
    use super::image_types::ImageProgress;
    if client.pin.manifest_sha256 != image.identity().manifest_sha256 {
        return Err(ImageError::Invalid("image client pin"));
    }
    let total = image.assets().iter().try_fold(0u64, |n, a| {
        n.checked_add(a.bytes()).ok_or(ImageError::LimitExceeded)
    })?;
    progress(ImageProgress::Verifying);
    let mut reconciled = false;
    let tx = loop {
        let (store, descriptor, stop) = (cache.clone(), image.clone(), closed.clone());
        if let Some(set) =
            blocking(move || store.verified_checked(&descriptor, Some(&stop))).await?
        {
            progress(ImageProgress::Download {
                bytes: total,
                total,
            });
            progress(ImageProgress::Verified);
            return Ok(set);
        }
        let (store, descriptor, stop) = (cache.clone(), image.clone(), closed.clone());
        match blocking(move || {
            super::image_cache::verification::cancelled(&stop)?;
            store.begin(&descriptor)
        })
        .await
        {
            Ok(owner) => break std::sync::Arc::new(owner.with_cancellation(closed.clone())),
            // Another owner may commit between the optimistic read and try_lock.
            Err(ImageError::RetainedImage) if !reconciled => reconciled = true,
            Err(error) => return Err(error),
        }
    };
    // The disk decodes from chunks as they land, so it is ready when the download ends.
    let owner = tx.clone();
    let (decoder, decoding) = if blocking(move || owner.needs_disk()).await? {
        let (decoder, receive) = std::sync::mpsc::sync_channel(16);
        let (owner, stop) = (tx.clone(), closed.clone());
        let source = Received {
            receive,
            bytes: Vec::new(),
            offset: 0,
        };
        // Its own thread: it waits on writes that need the blocking pool.
        let (done, job) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            let decoded = owner.decode(source, &stop);
            // Release the cache before the result, so a returned acquisition owns nothing.
            drop(owner);
            done.send(decoded)
        });
        (Some(decoder), Some(job))
    } else {
        (None, None)
    };
    let mut done = 0;
    progress(ImageProgress::Download { bytes: done, total });
    let fed: Result<(), ImageError> = async {
        for (index, asset) in image.assets().iter().enumerate() {
            let (owner, stop) = (tx.clone(), closed.clone());
            let pending = blocking(move || {
                super::image_cache::verification::cancelled(&stop)?;
                owner.start_asset(index)
            })
            .await?;
            let chunk = matches!(asset.role(), super::image_types::ImageAssetRole::Chunk(_));
            let decoder = decoder.clone().filter(|_| chunk);
            if let (None, Some(decoder)) = (&pending, decoder.clone()) {
                // Kept from an earlier run and rehashed above; the decoder starts over.
                let owner = tx.clone();
                blocking(move || owner.replay(index, &decoder)).await?;
            }
            if let Some(file) = pending {
                let mut file = asset_writer(file, tx.clone(), decoder);
                let url = format!("{}{}", asset.location().base(), asset.location().leaf());
                client
                    .transfer(
                        &url,
                        &mut file,
                        asset.bytes(),
                        (Some(asset.bytes()), asset.sha256()),
                        closed.clone(),
                        |bytes| {
                            progress(ImageProgress::Download {
                                bytes: done + bytes,
                                total,
                            })
                        },
                    )
                    .await?;
                drop(file);
                let (owner, stop) = (tx.clone(), closed.clone());
                blocking(move || {
                    super::image_cache::verification::cancelled(&stop)?;
                    owner.finish_asset(index)
                })
                .await?;
            }
            done += asset.bytes();
            progress(ImageProgress::Download { bytes: done, total });
        }
        Ok(())
    }
    .await;
    drop(decoder);
    if let Some(job) = decoding {
        let decoded = job
            .await
            .map_err(|_| ImageError::Storage(std::io::ErrorKind::Other))?;
        // A stopped decoder is why a write failed; otherwise the download failed first.
        if let (Err(ImageError::Storage(std::io::ErrorKind::BrokenPipe)) | Ok(()), Err(error)) =
            (&fed, decoded)
        {
            return Err(error);
        }
    }
    fed?;
    progress(ImageProgress::Verifying);
    let tx = std::sync::Arc::try_unwrap(tx).map_err(|_| ImageError::Busy)?;
    let set = blocking(move || tx.commit(&closed)).await?;
    progress(ImageProgress::Verified);
    Ok(set)
}

#[cfg(test)]
#[path = "image_http_tests.rs"]
mod tests;

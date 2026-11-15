//! Photos go straight from the door device to object storage (R2) with a
//! presigned PUT, so image bytes never pass through the API server.

use std::time::Duration;

use rusty_s3::actions::S3Action;
use rusty_s3::{Bucket, Credentials, UrlStyle};
use url::Url;

use crate::config::StorageConfig;

pub const UPLOAD_TTL: Duration = Duration::from_secs(15 * 60);
pub const READ_TTL: Duration = Duration::from_secs(60 * 60);

pub struct Storage {
    bucket: Bucket,
    credentials: Credentials,
    public_base: Option<Url>,
}

impl Storage {
    pub fn from_config(c: &StorageConfig) -> anyhow::Result<Option<Self>> {
        let (Some(endpoint), Some(bucket), Some(key), Some(secret)) =
            (&c.s3_endpoint, &c.s3_bucket, &c.s3_access_key_id, &c.s3_secret_access_key)
        else {
            return Ok(None);
        };
        let bucket = Bucket::new(Url::parse(endpoint)?, UrlStyle::Path, bucket.clone(), c.s3_region.clone())?;
        let public_base = c
            .s3_public_base_url
            .as_deref()
            .map(|u| Url::parse(&format!("{}/", u.trim_end_matches('/'))))
            .transpose()?;
        Ok(Some(Self { bucket, credentials: Credentials::new(key.clone(), secret.clone()), public_base }))
    }

    pub fn presign_put(&self, key: &str, content_type: &str) -> Url {
        let mut action = self.bucket.put_object(Some(&self.credentials), key);
        action.headers_mut().insert("content-type", content_type.to_string());
        action.sign(UPLOAD_TTL)
    }

    pub fn read_url(&self, key: &str) -> String {
        match &self.public_base {
            Some(base) => base.join(key).map(|u| u.to_string()).unwrap_or_default(),
            None => self.bucket.get_object(Some(&self.credentials), key).sign(READ_TTL).to_string(),
        }
    }
}

/// `events/<event>/photos/<photo>.jpg` — one prefix per event so a wedding's
/// photos can be exported or deleted together.
pub fn photo_key(event_id: uuid::Uuid, photo_id: uuid::Uuid, content_type: &str) -> String {
    let ext = match content_type {
        "image/png" => "png",
        "image/webp" => "webp",
        _ => "jpg",
    };
    format!("events/{event_id}/photos/{photo_id}.{ext}")
}

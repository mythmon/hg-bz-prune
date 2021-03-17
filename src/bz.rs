use anyhow::Context;
use serde::Deserialize;
use std::collections::HashMap;
use thiserror::Error;

const BZ_API: &str = "https://bugzilla.mozilla.org/rest";

#[derive(Error, Debug)]
pub enum BzError {
    #[error("The API did not return the expected information")]
    ApiFault,

    #[error("Could not complete API request")]
    HttpError(#[from] reqwest::Error),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

type Result<T> = std::result::Result<T, BzError>;

pub struct Bug {
    id: String,
}

impl Bug {
    pub fn new(id: String) -> Self {
        Self { id }
    }

    pub fn with_api<'a>(self, client: &'a reqwest::Client) -> ApiBug<'a> {
        ApiBug::new(client, self.id)
    }
}

#[derive(Debug, Deserialize)]
pub struct BugDetail {
    pub status: BugStatus,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "UPPERCASE")]
pub enum BugStatus {
    New,
    Resolved,
    Verified,
}

#[derive(Debug, Deserialize)]
pub struct Comment {
    pub id: u32,
    pub raw_text: String,
}

pub struct ApiBug<'a> {
    client: &'a reqwest::Client,
    id: String,
}

#[derive(Debug, Deserialize)]
struct ApiMapResponse<T> {
    bugs: HashMap<String, T>,
}

#[derive(Debug, Deserialize)]
struct ApiListResponse<T> {
    bugs: Vec<T>,
}

#[derive(Debug, Deserialize)]
struct BugComments {
    comments: Vec<Comment>,
}

impl<'a> ApiBug<'a> {
    fn new(client: &'a reqwest::Client, id: String) -> Self {
        Self { client, id }
    }

    pub async fn details(&self) -> Result<BugDetail> {
        let url = format!("{}/bug/{}", BZ_API, self.id);
        let res = self.client.get(url).send().await?;
        let mut data: ApiListResponse<BugDetail> = res
            .json()
            .await
            .context(format!("Failed to fetch details for bug {}", self.id))?;
        Ok(data
            .bugs
            .pop()
            .ok_or(BzError::ApiFault)
            .context("No bugs in response")?)
    }

    pub async fn comments(&self) -> Result<Vec<Comment>> {
        let url = format!("{}/bug/{}/comment", BZ_API, self.id);
        let res = self.client.get(url).send().await?;
        let mut data: ApiMapResponse<BugComments> = res
            .json()
            .await
            .context(format!("Failed to fetch comments for bug {}", self.id))?;
        Ok(data
            .bugs
            .remove(&self.id)
            .ok_or(BzError::ApiFault)
            .context("API fault: requested bug not in response")?
            .comments)
    }
}
